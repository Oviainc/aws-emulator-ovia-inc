Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# An application group is reported as an ordinary group: Group.ApplicationTag (awsApplication) is never minted and the group ARN carries no application id

_Filed: 2026-10-24T20:49:06Z_
_Operation: GetGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A group created with a service configuration of type `AWS::ResourceGroups::ApplicationGroup` is an APPLICATION group, and the model says two observable things about one:

1. `Group$ApplicationTag` — "A tag that defines the application group membership. This tag is only supported for application groups." The shape pins the content exactly: the map key must match `^awsApplication$` and the value must match `^arn:aws(-[a-z]+)*:resource-groups:<region>:<account>:group/[a-zA-Z0-9_\.-]{1,150}/[a-zA-Z0-9]{22,26}$`. So for an application group, real Resource Groups answers `Group.ApplicationTag = {"awsApplication": "<the group's ARN, including its application id segment>"}` — this is the tag callers then put on resources to make them members of the application (the myApplications / AppRegistry flow).
2. That same value pattern, plus `GroupName`'s second alternative (`[a-zA-Z0-9_\.-]{1,150}/[a-z0-9]{26}`) and the `TagSyncTask` ARN pattern (`group/<name>/<26 chars>/tag-sync-task/...`), show that an application group is identified as `<name>/<id>`, not as a bare name.

The emulator does neither. `GroupRecord::to_group` hardcodes `application_tag: None` for every group, and `CreateGroup` mints `group/<name>` with no id segment, so an application group is indistinguishable from a tag-query group on the wire.

Expected: creating a group whose `Configuration` contains an item of type `AWS::ResourceGroups::ApplicationGroup` yields a group whose `Group.ApplicationTag` is `{"awsApplication": "<group ARN>"}` (reported by CreateGroup, GetGroup and anywhere else a `Group` is serialized), with a group identity that carries the application id segment the `ApplicationArn` pattern requires.
Actual: 200 with `ApplicationTag` absent from every `Group` structure, and an ARN of the form `arn:aws:resource-groups:<region>:<account>:group/<name>` which does not satisfy `ApplicationArn`, so no client can derive the application tag for the group it just created.

## Evidence

Spec: `smithy_query(query="shape",shape="ApplicationTag")` — key `ApplicationTagKey` pattern `^awsApplication$`, value `ApplicationArn` pattern `^arn:aws(-[a-z]+)*:resource-groups:...:group/[a-zA-Z0-9_\.-]{1,150}/[a-zA-Z0-9]{22,26}$`; `smithy_query(query="doc",shape="Group",member="ApplicationTag")` — "A tag that defines the application group membership. This tag is only supported for application groups." `GroupConfigurationType` admits `AWS::ResourceGroups::ApplicationGroup` and `GroupResources`'s own documentation names it as one of the three group types it supports.

Handler/state: `src/state.rs` `GroupRecord::to_group()` (around lines 285-298) sets `application_tag: None` unconditionally, with the comment "Only an application group ... carries one, and this slice creates none"; `src/ops/create_group.rs` (around line 302) mints the ARN with `crate::state::group_arn(&region, &input.name)`, i.e. `group/<name>` with no application-id segment, whatever `Configuration` the request carried. `src/ops/get_group.rs` returns `group.to_group()` verbatim.

Tests: no suite mentions `ApplicationTag` (grep over `tests/` finds no occurrence); `tests/all/group_owner_configuration_and_update.rs` exercises configured groups and the Criticality/Owner/DisplayName members without asserting anything about the application tag or the group's identity form, so nothing pins the documented behaviour.

## Steps to Reproduce

1. POST /groups with {"Name":"myapp","Configuration":[{"Type":"AWS::ResourceGroups::ApplicationGroup"}],"DisplayName":"My App","Criticality":1}
2. Observe the response `Group`: no `ApplicationTag` member, and `GroupArn` is `arn:aws:resource-groups:<region>:000000000000:group/myapp` (no application-id segment, so it does not match the modelled `ApplicationArn` pattern)
3. POST /get-group with {"Group":"myapp"} and observe the same `Group` — still no `ApplicationTag`, so a caller cannot learn the `awsApplication` tag value to put on the resources it wants in the application
4. Expected in both: `"ApplicationTag": {"awsApplication": "<the group's ARN>"}` with the group's identity carrying its application id
