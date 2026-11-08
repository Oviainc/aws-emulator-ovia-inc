// SPDX-License-Identifier: CC0-1.0

//! The resource a tagging call of this service addresses, read off the request's own body.
//!
//! RULE. `TagResource`, `UntagResource` and `ListTagsForResource` each name their target in
//! the body member `resourceARN`
//! (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
//! binds it in the body, with no `jsonName` override, so that is its wire name), and nothing
//! else in such a request names the resource: these three operations bind no URI label.
//!
//! TWO readers need it, which is why it is here rather than inlined in either:
//!
//! * this service's authorization gate (`crate::service::authorization`), which decides the
//!   action against the resource the request ADDRESSED, and for these three operations that
//!   is whatever family the ARN in the body belongs to;
//! * the Bedrock control plane's tagging front door, which claims the three paths both
//!   models bind identically and reads this same member to tell a request about a Bedrock
//!   Data Automation resource from one about a resource of its own.
//!
//! Read as the `restJson1` document the body is, through the maintained JSON codec rather
//! than by hand, and read BEFORE either model's input schema is applied: for the front door
//! this member decides WHICH model's schema the request is then validated against, so it
//! cannot be taken through one of them.

use ::aws_smithy_runtime_api::http::Request as RtRequest;

/// The body member every tagging operation of this service carries its target in.
pub const RESOURCE_ARN_MEMBER: &str = "resourceARN";

/// The ARN the request's body names as the target of a tagging call, or `None` when the
/// body is not a readable JSON object carrying [`RESOURCE_ARN_MEMBER`] as a string.
///
/// `None` is the honest answer for a malformed or absent member: the operation the request
/// names refuses it itself, through the modelled constraint walk.
#[must_use]
pub fn resource_arn_in_body(req: &RtRequest) -> Option<String> {
    let body = req.body().bytes()?;
    let document: ::serde_json::Value = ::serde_json::from_slice(body).ok()?;
    document
        .get(RESOURCE_ARN_MEMBER)
        .and_then(::serde_json::Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::aws_smithy_types::body::SdkBody;

    /// A `POST` carrying `body`, as a client sends a tagging call.
    fn request(body: &str) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::from(body.to_string()));
        req.set_uri("/tagResource").expect("the test URI is valid");
        req.set_method("POST").expect("POST is a valid method");
        req
    }

    /// RULE: the target is the body's `resourceARN` member, read only where the body really
    /// carries one as a string.
    ///
    /// The rows span a well-formed target, a target absent from the body, a target of the
    /// wrong JSON type, a body that is not an object, and an unreadable body.
    #[test]
    fn the_target_is_read_off_the_body_when_the_body_carries_one() {
        let arn = "arn:aws:bedrock:us-west-2:000000000000:blueprint/83f78afe19be";
        let rows: [(&str, String, Option<&str>); 5] = [
            (
                "a tagging call naming its target",
                format!("{{\"resourceARN\":\"{arn}\",\"tags\":[]}}"),
                Some(arn),
            ),
            ("a body naming no target", "{\"tags\":[]}".to_string(), None),
            (
                "a target of another JSON type",
                "{\"resourceARN\":42}".to_string(),
                None,
            ),
            ("a body that is not an object", "[]".to_string(), None),
            ("a body no codec can read", "{".to_string(), None),
        ];
        for (case, body, expected) in rows {
            let actual = resource_arn_in_body(&request(&body));
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
