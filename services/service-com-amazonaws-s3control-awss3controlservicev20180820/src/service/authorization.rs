// SPDX-License-Identifier: CC0-1.0

//! Which account a request names, and whether the caller is authorized for it.
//!
//! Every operation of this service is addressed IN an account: its `AccountId`
//! member is bound to the `{AccountId}.` host prefix of the endpoint it is sent
//! to and to the `x-amz-account-id` header beside it
//! (`smithy_query(query="operation", operation="GetAccessPoint")`: `AccountId`
//! is `@required`, `hostLabel`, `httpHeader: x-amz-account-id`, and the
//! operation's `endpoint.hostPrefix` is `{AccountId}.`). That account is not
//! part of the resource the request addresses — it names WHOSE control plane the
//! request is for, so it is resolved and authorized BEFORE the request is
//! carried out, which is what makes a request for another account's control
//! plane the same refusal whatever it asks for, whether or not the resource it
//! names exists, and whatever the account text looks like.
//!
//! A caller is authorized for exactly ONE account: the one whose credentials
//! sign its requests, which for this emulator is [`caller_account`] — the single
//! account every emulated service presents ([`::engine::emulator_account_id`]).
//! A request naming any other account is a request for a control plane this
//! caller has no credentials for, and is refused with
//! [`crate::state::ACCESS_DENIED_CODE`] before the operation it names runs:
//! nothing is read, so the refusal reveals nothing about what the named account
//! holds, and nothing is written, so a request for another account cannot change
//! this one.
//!
//! The decision is read off the REQUEST rather than off a deserialized member,
//! for the same reason the access point name rules are
//! ([`super::envelope::Dispatch`]): it is answered ahead of the boundary's own
//! shape validation, so an account that could not be authorized is an
//! authorization failure rather than a shape violation — which is what a client
//! branching on the status and the code reads.

use aws_smithy_runtime_api::http::Request as RtRequest;

/// The header every operation of this service binds its `AccountId` member to.
pub const ACCOUNT_ID_HEADER: &str = "x-amz-account-id";

/// The header a request states the authority it was sent to in, when its URI
/// carries no authority of its own.
const HOST_HEADER: &str = "host";

/// The label this service's own endpoint begins with
/// (`smithy_query(query="service_info")`: `awsService.endpointPrefix` =
/// `s3-control`). The host prefix the account travels in sits immediately ahead
/// of it, so a label before THIS one is an account and a label before anything
/// else is not.
const ENDPOINT_PREFIX: &str = "s3-control";

/// The account this emulator's caller is authorized for: the single account
/// every emulated service presents, and the one every ARN it emits carries.
pub fn caller_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The host of the authority `uri` carries, for a URI stated in absolute form,
/// and `None` for the origin-form URI (`/v20180820/…`) a request usually
/// carries.
///
/// Parsed with [`::http::Uri`] rather than by splitting the string: a request
/// URI carries caller-supplied text in its query (an S3 URI among it), which a
/// hand-rolled split on `://` reads as an authority.
fn uri_authority(uri: &str) -> Option<String> {
    let parsed: ::http::Uri = uri.parse().ok()?;
    Some(parsed.authority()?.host().to_string())
}

/// The account the host prefix of `authority` names, or `None` for an authority
/// that carries no host prefix — the emulator's own host, and the service
/// endpoint addressed without one alike.
fn host_prefix_account(authority: &str) -> Option<&str> {
    let (label, rest) = authority.split_once('.')?;
    if label.is_empty() || !rest.starts_with(ENDPOINT_PREFIX) {
        return None;
    }
    Some(label)
}

/// Every account `req` names: the one its `x-amz-account-id` header carries —
/// which is the value the operation's own `AccountId` member is bound from — and
/// the one the host prefix of the endpoint it was sent to carries. A client
/// names its own account in both, so a request whose two differ names at least
/// one account that is not the caller's.
///
/// Empty when the request names no account at all, which is not an
/// authorization failure but an absent required member: the boundary answers it,
/// naming the member the caller has to send.
pub fn named_accounts(req: &RtRequest) -> Vec<String> {
    let authority = uri_authority(req.uri())
        .or_else(|| req.headers().get(HOST_HEADER).map(str::to_string));
    let from_host = authority
        .as_deref()
        .and_then(host_prefix_account)
        .map(str::to_string);
    [
        req.headers().get(ACCOUNT_ID_HEADER).map(str::to_string),
        from_host,
    ]
    .into_iter()
    .flatten()
    .filter(|account| !account.is_empty())
    .collect()
}

/// An account `req` names that the caller is NOT authorized for, and `None` when
/// every account it names is the caller's own or it names none.
pub fn unauthorized_account(req: &RtRequest) -> Option<String> {
    named_accounts(req)
        .into_iter()
        .find(|account| account != caller_account())
}

#[cfg(test)]
mod tests {
    //! The resolution rule read off requests. The crate's integration suite
    //! drives the refusal itself end to end over HTTP.

    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// A request for `path` carrying `headers`.
    fn request(path: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(path).expect("a path is a valid URI");
        for (name, value) in headers {
            req.headers_mut().append(name.to_string(), value.to_string());
        }
        req
    }

    /// One row of the resolution table: the case, the request URI, the headers
    /// it carries, and the accounts it names.
    type Row = (&'static str, &'static str, Vec<(&'static str, &'static str)>, Vec<&'static str>);

    #[test]
    fn every_account_a_request_names_is_read_from_its_header_and_its_host_prefix() {
        let cases: [Row; 9] = [
            (
                "the header alone",
                "/v20180820/accesspoint",
                vec![(ACCOUNT_ID_HEADER, "111111111111")],
                vec!["111111111111"],
            ),
            (
                "a host prefix ahead of the service endpoint",
                "/v20180820/accesspoint",
                vec![(HOST_HEADER, "222222222222.s3-control.us-west-2.amazonaws.com")],
                vec!["222222222222"],
            ),
            (
                "an absolute-form URI carrying the host prefix and no header",
                "https://333333333333.s3-control.us-west-2.amazonaws.com/v20180820/accesspoint",
                vec![],
                vec!["333333333333"],
            ),
            (
                "a header and a host prefix naming different accounts",
                "https://333333333333.s3-control.us-west-2.amazonaws.com/v20180820/accesspoint",
                vec![(ACCOUNT_ID_HEADER, "444444444444")],
                vec!["444444444444", "333333333333"],
            ),
            (
                "a host with no prefix, so the header names the account",
                "/v20180820/accesspoint",
                vec![
                    (HOST_HEADER, "s3-control.us-west-2.amazonaws.com"),
                    (ACCOUNT_ID_HEADER, "555555555555"),
                ],
                vec!["555555555555"],
            ),
            (
                "the emulator's own host, so the header names the account",
                "/v20180820/accesspoint",
                vec![
                    (HOST_HEADER, "127.0.0.1:9297"),
                    (ACCOUNT_ID_HEADER, "666666666666"),
                ],
                vec!["666666666666"],
            ),
            (
                "an account the shape would refuse is still the account named",
                "/v20180820/accesspoint",
                vec![(ACCOUNT_ID_HEADER, "notanaccountid")],
                vec!["notanaccountid"],
            ),
            (
                "no account at all",
                "/v20180820/accesspoint",
                vec![(HOST_HEADER, "127.0.0.1:9297")],
                vec![],
            ),
            (
                // A query member carrying an S3 URI: the `://` in it is not an
                // authority, so the host is still read from the header.
                "a query carrying a URI of its own",
                "/v20180820/accessgrantsinstance/dataaccess?target=s3%3A%2F%2Fbucket%2F%2A",
                vec![
                    (HOST_HEADER, "777777777777.s3-control.us-west-2.amazonaws.com"),
                    (ACCOUNT_ID_HEADER, "777777777777"),
                ],
                vec!["777777777777", "777777777777"],
            ),
        ];
        for (case, path, headers, expected) in cases {
            let req = request(path, &headers);
            let named = named_accounts(&req);
            assert_eq!(
                named, expected,
                "case {case:?}: expected the named accounts {expected:?}, got {named:?}"
            );
        }
    }

    #[test]
    fn only_the_callers_own_account_is_authorized() {
        let own = caller_account().to_string();
        let cases = [
            ("the caller's own account", own.as_str(), None),
            ("another account", "111111111111", Some("111111111111")),
            ("an account that is not one", "notanaccountid", Some("notanaccountid")),
        ];
        for (case, account, expected) in cases {
            let req = request("/v20180820/accesspoint", &[(ACCOUNT_ID_HEADER, account)]);
            let refused = unauthorized_account(&req);
            assert_eq!(
                refused.as_deref(),
                expected,
                "case {case:?}: expected {expected:?} to be refused, got {refused:?}"
            );
        }
    }
}
