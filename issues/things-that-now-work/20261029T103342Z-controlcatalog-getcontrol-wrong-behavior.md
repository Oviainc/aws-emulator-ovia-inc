Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9fa26429e9fb75f482c46ca7a1dcc182c128efe3

# GetControl reports Parameters named after the implementing rule's own settings, outside the five control-parameter types the model documents — and derives ParameterRequirementSummary from them

_Filed: 2026-10-29T10:33:42Z_
_Operation: GetControl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9fa26429e9fb75f482c46ca7a1dcc182c128efe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modeled `ControlParameter` structure — the very shape `GetControl`'s `Parameters` list is built from — documents exactly which control parameters exist: "Five types of control parameters are supported", enumerated as `AllowedRegions`, `ExemptedActions`, `ExemptedPrincipalArns`, `ExemptedResourceArns` and `ExemptAssumeRoot`, each described as a value a caller supplies when ENABLING the control (the doc's own example is an `EnableControl` request body with `"parameters": [ { "key": "ExemptAssumeRoot", ... } ]`).

The seeded catalog this handler projects, however, fills `parameters` for five of its controls with the implementing service's own rule settings instead: `maxAccessKeyAge`, `kmsId`, `trafficType`, `s3BucketName`/`snsTopicArn`/`cloudWatchLogsLogGroupArn`/`includeManagementEvents`/`readWriteType`, and `MinimumPasswordLength`/`RequireUppercaseCharacters`/`RequireNumbers`/`MaxPasswordAge`. `GetControl` serves them verbatim (`src/ops/get_control.rs:254-263`), so a client reading `Parameters` to build the `EnableControl` call the modeled doc describes is handed keys that are not control parameters at all.

The damage is not confined to the names: `ParameterRequirementSummary` is DERIVED from exactly that list (`published_controls::parameter_requirement_summary`, state.rs:919-921), and the model says that member is there so "you need [to] determine whether you need to supply parameter values when you enable the control". The control seeded from slug `access-keys-rotated` therefore answers `ParameterRequirementSummary: REQUIRED` on the strength of a Config-rule setting (`maxAccessKeyAge`), telling a caller it MUST supply a parameter value that the documented five-member vocabulary has no key for; the same control would summarise as `NONE` if only control parameters were counted.

The crate is also inconsistent with itself about which vocabulary a row uses: two rows DO use the modeled set (`ExemptedPrincipalArns` on `restrict-root-user-access-keys`, `AllowedRegions` + `ExemptedResourceArns` on `require-encrypted-ebs-volumes`), while five rows use the implementing rule's settings — so one control's `Parameters` are enable-time control parameters and the next control's are the backing rule's configuration, with nothing on the wire distinguishing the two.

## Evidence

Model, `smithy_query(query="doc", shape="ControlParameter")` — the shape `GetControlResponse$Parameters` is a list of:

> "Five types of control parameters are supported." … "**AllowedRegions**: List of Amazon Web Services Regions exempted from the control. … This parameter is mandatory for the **OU Region deny** control, **CT.MULTISERVICE.PV.1**." … "**ExemptedActions**: List of Amazon Web Services IAM actions exempted from the control." … "**ExemptedPrincipalArns**: …" … "**ExemptedResourceArns**: …" … "**ExemptAssumeRoot**: … This parameter applies only to the `AWS-GR_RESTRICT_ROOT_USER` control. … Example: `{ "controlIdentifier": "arn:aws:controlcatalog:::control/5kvme4m5d2b4d7if2fs5yg2ui", "parameters": [ { "key": "ExemptAssumeRoot", "value": true } ], "targetIdentifier": … }`"

And `GetControlResponse$ParameterRequirementSummary` (quoted in the handler's own header, services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/get_control.rs:93-96):
```
//! * `ParameterRequirementSummary` (ParameterRequirementSummary, one of: NONE | OPTIONAL |
//!   REQUIRED) — A summary that indicates whether the control requires parameters, accepts
//!   optional parameters, or does not support parameters. Use this field to determine whether
//!   you need to supply parameter values when you enable the control.
```

The crate's own rule for the member, src/state/published_controls.rs:33-36:
```
//! * `parameters` are the implementing rule's own parameters; the modeled
//!   `ParameterRequirementSummary` is DERIVED from them (see
//!   [`parameter_requirement_summary`]) rather than written per row, because it is
//!   defined as a summary of exactly that list.
```

Rows that follow that rule instead of the modeled vocabulary — src/state/published_controls.rs:499-501:
```
        // The Config rule's own parameter, which it has no default for: supplying it is
        // what makes this control's ParameterRequirementSummary REQUIRED.
        parameters: &[("maxAccessKeyAge", REQUIRED)],
```
src/state/published_controls.rs:297-305:
```
        // The Config rule's own parameters, all optional — which is what makes this
        // control's ParameterRequirementSummary OPTIONAL.
        parameters: &[
            ("s3BucketName", OPTIONAL),
            ("snsTopicArn", OPTIONAL),
            ("cloudWatchLogsLogGroupArn", OPTIONAL),
            ("includeManagementEvents", OPTIONAL),
            ("readWriteType", OPTIONAL),
        ],
```
src/state/published_controls.rs:525-530 (`MinimumPasswordLength`, `RequireUppercaseCharacters`, `RequireNumbers`, `MaxPasswordAge`), :555 (`parameters: &[("kmsId", OPTIONAL)]`), :579 (`parameters: &[("trafficType", OPTIONAL)]`).

The same table using the MODELED vocabulary two rows away — src/state/published_controls.rs:852 and :951-954:
```
        parameters: &[("ExemptedPrincipalArns", OPTIONAL)],
```
```
        parameters: &[
            ("AllowedRegions", REQUIRED),
            ("ExemptedResourceArns", OPTIONAL),
        ],
```

The handler serving them unchanged, and the derived summary beside it — src/ops/get_control.rs:253-263:
```
            parameter_requirement_summary: control.parameter_requirement_summary.clone(),
            parameters: Some(
                control
                    .parameters
                    .iter()
                    .map(|parameter| ControlParameter {
                        name: parameter.name.clone(),
                        requirement: parameter.requirement.clone(),
                    })
                    .collect(),
            ),
```
and the derivation site — src/state.rs:917-921:
```
                // A summary OF the parameters below, never an independent fact about
                // the control.
                parameter_requirement_summary: Some(
                    published_controls::parameter_requirement_summary(seed.parameters).to_string(),
                ),
```

Test coverage: tests/all/published_controls.rs:191 `parameter_requirement_summary_summarises_the_parameter_list` pins only the NONE/OPTIONAL/REQUIRED derivation from whatever names the list holds (tests/all/published_controls.rs:201-226 reads `control["Parameters"]` and only inspects each entry's `Requirement`); no test pins the parameter NAMES, so correcting the vocabulary leaves the existing assertions intact.

## Steps to Reproduce

1. Read the modeled contract for the shape GetControl's `Parameters` list is made of: `smithy_query(query="doc", shape="ControlParameter", model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")` — "Five types of control parameters are supported": AllowedRegions, ExemptedActions, ExemptedPrincipalArns, ExemptedResourceArns, ExemptAssumeRoot, each supplied when enabling the control.
2. Open services/service-com-amazonaws-controlcatalog-controlcatalog/src/state/published_controls.rs:33-36 and note the crate's rule: "`parameters` are the implementing rule's own parameters".
3. Read the rows at lines 501, 299-305, 525-530, 555 and 579: the parameter names served are `maxAccessKeyAge`, `s3BucketName`, `snsTopicArn`, `cloudWatchLogsLogGroupArn`, `includeManagementEvents`, `readWriteType`, `MinimumPasswordLength`, `RequireUppercaseCharacters`, `RequireNumbers`, `MaxPasswordAge`, `kmsId`, `trafficType` — none of them one of the five.
4. Compare with lines 852 and 951-954, where the same table DOES use the modeled vocabulary (`ExemptedPrincipalArns`, `AllowedRegions`, `ExemptedResourceArns`): two rows speak the model's parameter language and five speak the backing rule's.
5. Call ListControls and pick the control whose `Name` is "Checks whether an IAM access key is rotated within the required number of days" (seeded from slug `access-keys-rotated`, published_controls.rs:487-501), then call GetControl with its `Arn`: the response carries `Parameters: [{"Name": "maxAccessKeyAge", "Requirement": "REQUIRED"}]` and `ParameterRequirementSummary: "REQUIRED"` (src/ops/get_control.rs:253-263), i.e. it tells the caller a parameter value must be supplied when enabling the control, under a key the documented five-member set has no entry for.
