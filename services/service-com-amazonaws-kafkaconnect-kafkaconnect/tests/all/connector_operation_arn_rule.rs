// SPDX-License-Identifier: CC0-1.0

//! The ONE statement, for this crate's tests, of the ARN a started connector
//! operation must be reported under.
//!
//! Shared by every suite that checks it, so no two can state the rule
//! differently, and derived from the ARN GRAMMAR AWS publishes — six
//! `:`-separated fields, of which the last keeps every `:` it carries, with the
//! resource field's TYPE ending at its first separator — rather than from the
//! service's own derivation. A resource NAME is caller-supplied text, so this
//! reads no text inside the name as a boundary and a handler that does fails
//! against it.

/// The resource type a connector's ARN opens its resource field with.
pub const CONNECTOR_TYPE: &str = "connector";

/// The resource type an operation's ARN opens its resource field with.
pub const OPERATION_TYPE: &str = "connector-operation";

/// The ARN `value`'s five leading fields (joined as they were submitted) and its
/// whole resource field.
pub fn scope_and_resource(case: &str, value: &str) -> (String, String) {
    let mut fields = value.splitn(6, ':');
    let leading: Vec<&str> = fields.by_ref().take(5).collect();
    let resource = fields.next();
    assert!(
        leading.len() == 5 && resource.is_some() && leading[0] == "arn",
        "case {case:?}: expected a six-field ARN, got {value:?}"
    );
    (leading.join(":"), resource.unwrap_or_default().to_string())
}

/// The ARN an operation of id `operation_id` against `connector_arn` must be
/// reported under: the connector ARN's five leading fields, and its resource
/// field with the leading resource type replaced and `operation_id` appended.
pub fn derived_operation_arn(case: &str, connector_arn: &str, operation_id: &str) -> String {
    let (scope, resource) = scope_and_resource(case, connector_arn);
    let named = resource
        .strip_prefix(CONNECTOR_TYPE)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {CONNECTOR_TYPE}/ resource, got {connector_arn:?}")
        });
    format!("{scope}:{OPERATION_TYPE}/{named}/{operation_id}")
}

/// The last `/`-separated segment of `arn`: the operation's own minted id, which
/// carries no `/` of its own.
pub fn last_segment(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or_default()
}
