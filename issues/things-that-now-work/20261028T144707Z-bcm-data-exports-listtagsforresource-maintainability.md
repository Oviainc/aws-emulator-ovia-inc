Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 15aaa5471fc3f7278352228b97d2216e65bea584

# ARN field splitting is hand-rolled twice in this crate, where service-common's arn::parse is declared the tree's one parse

_Filed: 2026-10-28T14:47:07Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `15aaa5471fc3f7278352228b97d2216e65bea584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate splits an ARN into its six colon-separated fields in two separate hand-rolled places:

1. `state::validate_taggable_resource_arn` reads the account segment with `arn.split(':').nth(ARN_ACCOUNT_SEGMENT)` plus its own `const ARN_ACCOUNT_SEGMENT: usize = 4`.
2. `state::authorization::arn_resource` re-implements the whole parse — `splitn(6, ':')`, the literal `"arn"` check, and the six-field destructuring — to pull the resource field out of an STS/IAM principal ARN.

`service_common::arn::parse` already returns exactly those six fields and explicitly claims to be the single implementation ("The ONE parse: a service that needs more than the vendor ... reads it from here rather than splitting the value again, so no two services can disagree about what an ARN is"). `service-common` is already a dependency of this crate (it supplies `iam_authorization` for `authorize` and `validation::unanchored` for the refusal renderer), and this crate's authorization module already applies that very principle for policy ARNs ("Read through IAM's own ARN parser, so this service and IAM agree on what an AWS-managed policy ARN is"). The two local copies can drift from each other and from the shared parse: today they disagree on what counts as well-formed (`arn_resource` requires the literal `arn` and six fields; the tagging check requires neither, so a five-field value silently reads some other segment as the account).

Fix: have both read `::service_common::arn::parse` — `parse(arn).is_some_and(|parsed| parsed.account == ::engine::emulator_account_id())` for the tagging check, and `parse(arn).filter(|parsed| parsed.vendor == service).map(|parsed| parsed.resource)` for `arn_resource`. Every ARN that reaches either site has already passed the model's `@pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$` (tagging) or was minted by STS/IAM (authorization), so every response and every existing test stays byte-identical.

## Evidence

The shared parse's own claim to be the only one — services/common/src/arn.rs:82-108:
```
/// `value`'s six fields, or `None` when `value` is not a well-formed resource ARN.
///
/// Well formed here means all six fields are present, the first is literally `arn`,
/// and the partition, the vendor and the resource are non-empty. The ONE parse: a
/// service that needs more than the vendor (the region an ARN names, the resource type
/// it addresses) reads it from here rather than splitting the value again, so no two
/// services can disagree about what an ARN is.
#[must_use]
pub fn parse(value: &str) -> Option<Arn<'_>> {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let mut fields = value.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    let vendor = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
```
(and `pub mod arn;` at services/common/src/lib.rs:203).

Copy 1 — services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state.rs:804-818:
```
pub fn validate_taggable_resource_arn(arn: &str) -> Result<(), ValidationException> {
    let names_this_account = arn
        .split(':')
        .nth(ARN_ACCOUNT_SEGMENT)
        .is_some_and(|account| account == ::engine::emulator_account_id());
...
/// The 0-based position of the account segment in an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
const ARN_ACCOUNT_SEGMENT: usize = 4;
```

Copy 2 — services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state/authorization.rs:276-290:
```
/// The resource part of an ARN whose service segment is `service`.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(named), Some(_region), Some(_account), Some(resource)) = (
        fields.next(), fields.next(), fields.next(), fields.next(), fields.next(), fields.next(),
    ) else {
        return None;
    };
    (named == service).then_some(resource)
}
```

The crate already applies the "read it from the one owner" rule next door — authorization.rs:247-254:
```
/// Whether `arn` names an AWS-MANAGED policy: ... Read through IAM's own ARN parser, so
/// this service and IAM agree on what an AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_store::parse_policy_arn(arn)
```
and already depends on the crate that owns `arn` — Cargo.toml:35-37:
```
# `iam_authorization`: the shared evaluation of IAM policy documents against
# (action, resource, principal), which this service's authorization check runs.
service-common = { path = "../common" }
```
(the crate also calls `::service_common::validation::unanchored` at src/service/validation.rs:77).

Tests that pin the behaviour both copies must keep: tests/all/tagging_arn_validation.rs, tests/all/untagging_arn_validation.rs (the cross-account `ResourceArn` -> 400 `Resource doesn't support tagging` rule that copy 1 decides), and tests/all/front_door_authorization.rs, tests/all/untagging_authorization.rs (the principal resolution that copy 2 decides).

## Steps to Reproduce

1. `grep -n "splitn(6, ':')\|split(':').nth" services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src` — two hand-rolled ARN field splits: state.rs:805-807 and state/authorization.rs:278-288.
2. `grep -n 'pub fn parse' services/common/src/arn.rs` — the shared six-field parse the crate could call, in a crate already listed in this crate's Cargo.toml (line 37).
3. Replace both with `::service_common::arn::parse`; re-run `cargo test -p service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports` and confirm tests/all/tagging_arn_validation.rs, tests/all/untagging_arn_validation.rs, tests/all/front_door_authorization.rs and tests/all/untagging_authorization.rs still pass unchanged.
