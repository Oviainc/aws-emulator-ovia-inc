Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3c3c87ac9166712e1ad44c6d25ef542a1ff1b247

# RELATED_CONTROL mapping reports COMPLEMENTARY for the CloudTrail pair that is the same check on two surfaces, which the table's own rule defines as ALTERNATIVE

_Filed: 2026-10-29T13:59:36Z_
_Operation: ListControlMappings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3c3c87ac9166712e1ad44c6d25ef542a1ff1b247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListControlMappings` projects each stored `RELATED_CONTROL` record's `RelationType` straight onto the wire (src/ops/list_control_mappings.rs:98-104), so the value the catalog seeds is the value a client reads. The seed table `RELATED_CONTROLS` states its own rule for choosing between the two relation types it uses, and one row breaks it.

The rule (src/state/published_controls.rs:994-1000): ALTERNATIVE is for a pair "where either control satisfies the common control the pair implements — the typical case for the same check published on two surfaces", and COMPLEMENTARY is for a pair "where both can be enabled and reinforce one another, as a preventive control does with the detective control that reports what slipped past it".

The row `("cloud-trail-enabled", "detect-cloud-trail-enabled-on-member-accounts", "COMPLEMENTARY")` (src/state/published_controls.rs:1017-1021) is exactly the ALTERNATIVE case the rule describes, not the COMPLEMENTARY one:

* both rows carry the SAME check — `implementation_type: CONFIG_RULE` with `implementation_identifier: Some("CLOUD_TRAIL_ENABLED")` (lines 241-242 and 267-268) — and the second row's own comment says so: "The same Config rule backs this Control Tower control and the Config-rule control above: two published controls, one implementation" (lines 265-266). That is the rule's "same check published on two surfaces";
* both `implements: "enable-centralized-logging"` (lines 250 and 276), so either control satisfies the common control the pair implements — the rule's test for ALTERNATIVE;
* both are `behavior: DETECTIVE` (lines 239 and 263), so the COMPLEMENTARY description's own example ("as a preventive control does with the detective control that reports what slipped past it") cannot apply to them.

Every other row in the table follows the rule: all four same-check-on-two-surfaces pairs are ALTERNATIVE, and every other COMPLEMENTARY pair joins controls with different checks and (bar the read/write S3 pair, which are two different checks) different behaviors. So this one row makes the catalog answer a different relation for the one pair the rule covers most explicitly, in both directions (the seeder records each row twice, src/state.rs:983-993).

The fix is the relation on that single row; no constant, bound or new value is involved.

## Evidence

The rule the table states for itself — services/service-com-amazonaws-controlcatalog-controlcatalog/src/state/published_controls.rs:991-1000:
```
/// How two published controls relate: `(control slug, related control slug,
/// ControlRelationType)`.
///
/// ALTERNATIVE where either control satisfies the common control the pair implements —
/// the typical case for the same check published on two surfaces — and COMPLEMENTARY
/// where both can be enabled and reinforce one another, as a preventive control does
/// with the detective control that reports what slipped past it. This catalog publishes
/// no pair that conflicts, so it records no MUTUALLY_EXCLUSIVE relation. The seeder
/// records each row in BOTH directions, because a query filters on one control at a
/// time and each side has to report the relation when it is the one queried.
pub(super) const RELATED_CONTROLS: &[(&str, &str, &str)] = &[
```

The row that breaks it — src/state/published_controls.rs:1017-1021:
```
    (
        "cloud-trail-enabled",
        "detect-cloud-trail-enabled-on-member-accounts",
        "COMPLEMENTARY",
    ),
```

The two rows it names are the same check on two surfaces, satisfying one common control — src/state/published_controls.rs:239-250:
```
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
...
        implements: "enable-centralized-logging",
```
and src/state/published_controls.rs:263-276:
```
        behavior: DETECTIVE,
        severity: HIGH,
        // The same Config rule backs this Control Tower control and the Config-rule
        // control above: two published controls, one implementation.
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: Some("AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS"),
...
        implements: "enable-centralized-logging",
```

The sibling rows that DO follow the rule, each a same-check-two-surfaces pair sharing one `implements` — src/state/published_controls.rs:1002-1016 and 1027-1037:
```
    (
        "s3-bucket-server-side-encryption-enabled",
        "securityhub-s3-4",
        "ALTERNATIVE",
    ),
...
    (
        "multi-region-cloud-trail-enabled",
        "securityhub-cloudtrail-1",
        "ALTERNATIVE",
    ),
...
    (
        "iam-root-access-key-check",
        "securityhub-iam-4",
        "ALTERNATIVE",
    ),
...
    ("iam-user-mfa-enabled", "securityhub-iam-5", "ALTERNATIVE"),
```

The seeder recording the row in both directions — src/state.rs:983-993:
```
    for &(one, other, relation_type) in published_controls::RELATED_CONTROLS {
        for (from, to) in [(one, other), (other, one)] {
            mapping_seed.push((
                control_arn(from),
                MappingDetails::RelatedControl {
                    control_arn: Some(control_arn(to)),
                    relation_type: relation_type.to_string(),
```

The handler putting it on the wire unchanged — src/ops/list_control_mappings.rs:98-104:
```
        crate::state::MappingDetails::RelatedControl {
            control_arn,
            relation_type,
        } => Mapping::RelatedControl(RelatedControlMappingDetails {
            control_arn: control_arn.clone(),
            relation_type: relation_type.clone(),
        }),
```

Test coverage: tests/all/control_mappings.rs:161-251 `every_mapping_kind_round_trips_through_its_own_filter` compares the wire against the stored records and only asserts that each relation is one of `RELATION_TYPES` and that at least two distinct relations are recorded (tests/all/control_mappings.rs:242-251), so changing this one row's relation keeps every existing assertion satisfied.

## Steps to Reproduce

1. Read the rule the seed table states for itself: services/service-com-amazonaws-controlcatalog-controlcatalog/src/state/published_controls.rs:991-1000 — ALTERNATIVE is "the typical case for the same check published on two surfaces" where "either control satisfies the common control the pair implements"; COMPLEMENTARY is for controls that "reinforce one another, as a preventive control does with the detective control".
2. Read src/state/published_controls.rs:234-283: `cloud-trail-enabled` and `detect-cloud-trail-enabled-on-member-accounts` both carry `implementation_type: CONFIG_RULE` + `implementation_identifier: Some("CLOUD_TRAIL_ENABLED")`, both `behavior: DETECTIVE`, both `implements: "enable-centralized-logging"`, and the second row's comment at lines 265-266 states they are "two published controls, one implementation".
3. Read src/state/published_controls.rs:1017-1021: the pair is recorded `"COMPLEMENTARY"`, while the four other same-check-two-surfaces pairs at lines 1002-1006, 1012-1016, 1027-1031 and 1037 are recorded `"ALTERNATIVE"`.
4. Start the emulator and call `POST /get-control` with the Control Tower ARN `arn:aws:controltower:us-east-1::control/AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS` to read that control's catalog `Arn`.
5. Call `POST /list-control-mappings` with body `{"Filter":{"ControlArns":["<that Arn>"],"MappingTypes":["RELATED_CONTROL"]}}`: the mapping to the `cloud-trail-enabled` control comes back with `"RelationType": "COMPLEMENTARY"`, where the table's own rule (and the relation reported for every other pair that is one check on two surfaces) is `ALTERNATIVE`. The reverse direction, filtered on the `cloud-trail-enabled` ARN, reports the same wrong relation.
