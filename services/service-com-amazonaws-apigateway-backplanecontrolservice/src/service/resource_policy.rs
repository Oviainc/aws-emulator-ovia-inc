// SPDX-License-Identifier: CC0-1.0

//! The api's own RESOURCE POLICY, as the `execute-api` data plane applies it to one invoke.
//!
//! `RestApi$policy` is "a stringified JSON policy document that applies to this RestApi
//! regardless of the caller and Method configuration", so a document a developer attached
//! is consulted for EVERY request a deployed stage answers — whatever the method's
//! `authorizationType` is — and a request it does not allow is refused before the
//! integration is reached. An api carrying no document has no resource-based grant to read
//! and is served exactly as it was before one existed.
//!
//! A second action the document decides is the one a request asking for the stage's CACHED
//! response to be reloaded asks for ([`INVALIDATE_CACHE_ACTION`]): a stage whose method
//! settings require authorization for a cache-control header reads that grant through
//! [`authorizes_invalidation`], and what it does with an unauthorized ask is the stage's
//! own `unauthorizedCacheControlHeaderStrategy` rather than this module's business.
//!
//! The DECISION is not made here: it is
//! [`::service_common::iam_authorization`], the tree's one evaluation of IAM policy
//! documents, so this data plane and every other service handed the same document decide
//! alike. What lives here is only what is particular to an invoke:
//!
//! * the ACTION it asks for ([`INVOKE_ACTION`]);
//! * the two spellings the RESOURCE element of such a document names a method by — the
//!   method's own ARN and the `execute-api:/{stage}/{verb}/{path}` form API Gateway's own
//!   policy examples are written in — where a value matching EITHER is about this call;
//! * the condition keys an invoke of this emulator can answer for ([`PUBLISHED_KEYS`]): the
//!   VPC endpoint the request arrived through, which is the one property of a request that
//!   this data plane reads off the endpoint host it was addressed on. A clause over any
//!   other key is one this evaluation cannot decide, and an undecided clause never grants
//!   while it still refuses;
//! * who the request speaks for: a request to a method that authorizes nobody speaks for
//!   nobody, which is the [`ANONYMOUS_PRINCIPAL`] a refusal names, and only a statement
//!   whose `Principal` is the whole-element `"*"` is about it ([`Caller`]).
//!
//! ## What an UNSTATED document does, and why that depends on the method
//!
//! An explicit `Deny` refuses whoever asked. A document that neither allows nor denies
//! refuses a request to a method that authorizes nobody ([`Caller::Anonymous`], the
//! `authorizationType` `NONE` case): API Gateway authenticates no caller for such a method,
//! so the resource policy is the only grant there is and without it the request is refused
//! — which is why a request to a PRIVATE api carrying no allow at all is answered `403`
//! whatever it signs with. A request to an `AWS_IAM` method ([`Caller::Signed`]) is not
//! refused on an unstated document: AWS decides that one against the caller's
//! identity-based policies as well, where either grant suffices, so refusing it here would
//! deny a call AWS permits. This emulator models no identity-based policies for an invoke
//! (see [`super::invoke::authorization_refusal`]), which is exactly why the unstated case
//! is read fail-OPEN there and fail-closed for a method that authorizes nobody.

use ::service_common::iam_authorization::{self as iam, Decision, PreparedDocument};

/// The IAM action an invoke of a deployed method asks for, which is the action an API
/// Gateway resource policy is written about: `execute-api:Invoke`.
pub const INVOKE_ACTION: &str = "execute-api:Invoke";

/// The IAM action a request asking for the stage's CACHED response to be reloaded asks
/// for: `execute-api:InvalidateCache`.
///
/// Invalidating a cached entry is a separate permission from invoking the method — a
/// caller that may invoke a method is not thereby allowed to drop the entry the stage
/// holds for it — which is what `MethodSetting$requireAuthorizationForCacheControl`
/// ("whether authorization is required for a cache invalidation request") turns on.
pub const INVALIDATE_CACHE_ACTION: &str = "execute-api:InvalidateCache";

/// The condition key naming the VPC endpoint a request arrived through — the key a private
/// api's policy is pinned to one of its interface endpoints with.
pub const SOURCE_VPCE_KEY: &str = "aws:SourceVpce";

/// The condition keys an invoke MODELS: the ones whose absence from the context means the
/// request really carries no value for them rather than that this emulator cannot answer
/// for them.
///
/// Only the source VPC endpoint, because the endpoint host a request was addressed on is
/// the one such property this data plane can read. Every other key AWS publishes for an
/// invoke (the source VPC, the source address, the caller's own tags) is one this
/// evaluation cannot answer, so a clause over it is left undecided rather than guessed at.
const PUBLISHED_KEYS: &[&str] = &[SOURCE_VPCE_KEY];

/// The principal a request to a method that authorizes nobody speaks for, as AWS names it
/// in the sentence it refuses such a request with.
pub const ANONYMOUS_PRINCIPAL: &str = "anonymous";

/// The service an invoke's resource ARN names: `arn:aws:execute-api:…`.
const ARN_SERVICE: &str = "execute-api";

/// Who the request speaks for, which is decided by the METHOD's `authorizationType` rather
/// than by what the request carries: API Gateway authenticates a caller for an `AWS_IAM`
/// method and nobody for a method whose authorization is `NONE`.
#[derive(Debug, Clone, Copy)]
pub enum Caller<'a> {
    /// The method authorizes nobody, so no statement naming a principal is about this
    /// request — only the whole-element `"*"` is — and the document is the only grant there
    /// can be.
    Anonymous,
    /// The method is authorized by IAM and the emulator verified the credential, so the
    /// request speaks for this principal — whose identity-based policies AWS evaluates
    /// beside the document.
    Signed(&'a str),
}

/// What one invoke asks the api's resource policy about.
///
/// Every field is read off the request and the records that resolved it — the stage and the
/// resource path the deployment matched, the verb the method is addressed by, the endpoint
/// host it arrived on — so the decision is about this call rather than about a
/// configuration.
#[derive(Debug)]
pub struct Call<'a> {
    /// The Region the request addressed, which is the Region of the method ARN a statement
    /// names and a refusal reports.
    pub region: &'a str,
    /// The api the request resolved to.
    pub api_id: &'a str,
    /// The stage within that api.
    pub stage: &'a str,
    /// The verb the deployed method is addressed by — the request's own, or the `ANY`
    /// catch-all it matched.
    pub verb: &'a str,
    /// The resource path of the DEPLOYED method the request matched, leading `/` included.
    pub resource_path: &'a str,
    /// The VPC endpoint the request arrived through, when the endpoint it was addressed on
    /// names one.
    pub source_vpce: Option<&'a str>,
    /// Who the request speaks for; see [`Caller`].
    pub caller: Caller<'a>,
}

impl Call<'_> {
    /// The ARN of the method this call addresses:
    /// `arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{verb}/{path}`.
    ///
    /// The path carries no leading `/` of its own — the ARN's separator is already there —
    /// so the root resource is the ARN ending in the verb and a slash, which is the form
    /// AWS reports it in.
    #[must_use]
    pub fn method_arn(&self) -> String {
        ::emulator_server::emulator_arn(
            ARN_SERVICE,
            self.region,
            &format!(
                "{api}/{stage}/{verb}/{path}",
                api = self.api_id,
                stage = self.stage,
                verb = self.verb,
                path = self.resource_path.trim_start_matches('/'),
            ),
        )
    }

    /// The same method in the form an API Gateway resource policy's own examples name it
    /// by: `execute-api:/{stage}/{verb}/{path}`, the spelling `execute-api:/*` covers.
    fn scoped_resource(&self) -> String {
        format!(
            "{ARN_SERVICE}:/{stage}/{verb}/{path}",
            stage = self.stage,
            verb = self.verb,
            path = self.resource_path.trim_start_matches('/'),
        )
    }

    /// The principal a refusal of this call names.
    fn principal_named(&self) -> &str {
        match self.caller {
            Caller::Anonymous => ANONYMOUS_PRINCIPAL,
            Caller::Signed(principal) => principal,
        }
    }

    /// Every spelling of the caller a statement's `Principal` element may name it by: the
    /// authenticated principal, and nothing at all for a method that authorizes nobody —
    /// where only the whole-element `"*"` is about the request.
    fn principals(&self) -> Vec<String> {
        match self.caller {
            Caller::Anonymous => Vec::new(),
            Caller::Signed(principal) => vec![principal.to_string()],
        }
    }
}

/// The sentence this call is refused with when `policy` does not let it invoke the method,
/// or `None` when the policy lets it through.
///
/// `policy` is the document the api carries; an api carrying none is not asked at all.
#[must_use]
pub fn denial(policy: &str, call: &Call<'_>) -> Option<String> {
    let refused = refuses(decision(policy, call, INVOKE_ACTION), call);
    refused.then(|| not_authorized(INVOKE_ACTION, call.principal_named(), &call.method_arn()))
}

/// Whether the document the api carries explicitly ALLOWS this call to invoke the method.
///
/// The one thing [`denial`] cannot report: it answers whether the document REFUSES the call,
/// and for an IAM-authorized caller an unstated document is not a refusal (the module docs give
/// the argument). Whether it is an ALLOW is a different question, and it is the one the
/// identity-based gate beside this one needs — for a caller in the api's own account either
/// grant suffices, so an allow here admits a caller whose own policies state nothing
/// ([`super::invoke::execute_api_authorization`]).
///
/// `policy` is `None` for an api carrying no document, which allows nothing.
#[must_use]
pub fn allows_invoke(policy: Option<&str>, call: &Call<'_>) -> bool {
    policy.is_some_and(|policy| decision(policy, call, INVOKE_ACTION) == Decision::Allowed)
}

/// Whether this call may ask for the stage's cached response to be reloaded, which is what
/// a stage requiring authorization for a cache-control header asks about.
///
/// `policy` is the document the api carries, `None` for an api carrying none — which
/// states nothing about the action, and so is read exactly as a document that mentions it
/// nowhere: a request to a method that authorizes nobody holds no grant at all, while an
/// IAM-authorized caller may hold an identity-based one this emulator does not model (the
/// module docs give the whole argument, and it is the same one [`denial`] reads an invoke
/// by).
#[must_use]
pub fn authorizes_invalidation(policy: Option<&str>, call: &Call<'_>) -> bool {
    let stated = policy.map_or(Decision::NotStated, |policy| {
        decision(policy, call, INVALIDATE_CACHE_ACTION)
    });
    !refuses(stated, call)
}

/// The sentence a cache invalidation this call is not authorized for is refused with: the
/// principal that asked, the action it asked for and the method it asked for, as
/// [`not_authorized`] words every such refusal.
#[must_use]
pub fn invalidation_not_authorized(call: &Call<'_>) -> String {
    not_authorized(
        INVALIDATE_CACHE_ACTION,
        call.principal_named(),
        &call.method_arn(),
    )
}

/// Whether what the document STATED about this call refuses it.
///
/// See the module docs: for a method that authorizes nobody the document is the only grant
/// there can be, so an unstated one refuses, while an IAM-authorized caller may hold an
/// identity-based grant this emulator does not model.
fn refuses(stated: Decision, call: &Call<'_>) -> bool {
    match stated {
        Decision::Denied => true,
        Decision::Allowed => false,
        Decision::NotStated => matches!(call.caller, Caller::Anonymous),
    }
}

/// What `policy` states about this call, over BOTH spellings of the resource a statement
/// may name the method by.
///
/// A `Deny` under either spelling refuses — an explicit deny wins over every grant,
/// whichever way the statement wrote the resource — and an `Allow` under either grants,
/// because the two spellings name ONE method and a document is not read as stating less
/// than it wrote.
fn decision(policy: &str, call: &Call<'_>, action: &str) -> Decision {
    let document = PreparedDocument::read(policy);
    let principals = call.principals();
    let context: Vec<(String, String)> = call
        .source_vpce
        .map(|endpoint| vec![(SOURCE_VPCE_KEY.to_string(), endpoint.to_string())])
        .unwrap_or_default();
    let mut allowed = false;
    for resource in [call.method_arn(), call.scoped_resource()] {
        let request = iam::Request::new(action, &resource, &principals)
            .with_context(&context)
            .with_published_keys(PUBLISHED_KEYS);
        match iam::resource_outcome_prepared(&document, &request).decision {
            Decision::Denied => return Decision::Denied,
            Decision::Allowed => allowed = true,
            Decision::NotStated => {}
        }
    }
    if allowed {
        Decision::Allowed
    } else {
        Decision::NotStated
    }
}

/// The sentence AWS refuses a call with when the api's policy grants the caller nothing
/// for the action it asked for: the principal that asked, that action and the method it
/// asked for.
///
/// Not `::service_common::iam_authorization::not_authorized_by_resource_policy`, whose
/// sentence ends in the clause OTHER services word their refusal with ("because no
/// resource-based policy allows …"); the `execute-api` endpoint's own answer ends at the
/// resource.
///
/// `pub(crate)` because the IDENTITY-based gate beside this one refuses the same call about
/// the same method in the same sentence ([`super::invoke::execute_api_authorization`]) — one
/// wording, so the two gates cannot diverge while their docs say they do not.
pub(crate) fn not_authorized(action: &str, principal: &str, method_arn: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {method_arn}")
}

#[cfg(test)]
mod tests {
    //! The two pure rules this module owns: the shape of the resource an invoke names, and
    //! which decision each kind of document reaches for it. What the data plane then SERVES
    //! is pinned end to end in `tests/all/private_api_resource_policy.rs`.

    use super::*;

    /// One invoke of `GET /whoami` on stage `s` of api `abc1230000`, arriving through
    /// `source_vpce` and speaking for `caller`.
    fn call<'a>(source_vpce: Option<&'a str>, caller: Caller<'a>) -> Call<'a> {
        Call {
            region: "us-west-2",
            api_id: "abc1230000",
            stage: "s",
            verb: "GET",
            resource_path: "/whoami",
            source_vpce,
            caller,
        }
    }

    #[test]
    fn a_call_names_its_method_in_both_spellings_a_statement_may_use() {
        let call = call(None, Caller::Anonymous);
        assert_eq!(
            call.method_arn(),
            "arn:aws:execute-api:us-west-2:000000000000:abc1230000/s/GET/whoami",
            "case the method ARN: got {:?}",
            call.method_arn()
        );
        assert_eq!(
            call.scoped_resource(),
            "execute-api:/s/GET/whoami",
            "case the scoped resource: got {:?}",
            call.scoped_resource()
        );

        // The root resource is the one path whose spelling is a bare `/`: the ARN and the
        // scoped form both end in the verb and a separator, with no segment after it.
        let root = Call {
            resource_path: "/",
            ..call
        };
        assert_eq!(
            root.method_arn(),
            "arn:aws:execute-api:us-west-2:000000000000:abc1230000/s/GET/",
            "case the root method ARN: got {:?}",
            root.method_arn()
        );
        assert_eq!(
            root.scoped_resource(),
            "execute-api:/s/GET/",
            "case the root scoped resource: got {:?}",
            root.scoped_resource()
        );
    }

    #[test]
    fn a_source_vpce_condition_decides_the_call_the_request_arrived_through() {
        const ENDPOINT: &str = "vpce-0123456789abcdef0";
        const OTHER: &str = "vpce-00000000000000000";
        let pinned = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":\"*\",\"Action\":\"execute-api:Invoke\",\
             \"Resource\":\"execute-api:/*\",\"Condition\":{{\"StringEquals\":\
             {{\"aws:SourceVpce\":\"{ENDPOINT}\"}}}}}}]}}"
        );
        // Rows differ along the endpoint the request arrived through, including arriving
        // through none at all — which is a request the pinned grant does not cover.
        let rows: &[(&str, Option<&str>, bool)] = &[
            ("the endpoint the policy names", Some(ENDPOINT), true),
            ("another endpoint", Some(OTHER), false),
            ("no endpoint at all", None, false),
        ];
        for (case, endpoint, allowed) in rows {
            let refusal = denial(&pinned, &call(*endpoint, Caller::Anonymous));
            assert_eq!(
                refusal.is_none(),
                *allowed,
                "case {case}: expected allowed={allowed}, got {refusal:?}"
            );
        }
    }

    #[test]
    fn the_documents_effect_and_the_methods_authorization_decide_an_unstated_policy() {
        let allow_all = "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\",\
                         \"Action\":\"execute-api:Invoke\",\"Resource\":\"execute-api:/*\"}]}";
        let deny_all = "{\"Statement\":[{\"Effect\":\"Deny\",\"Principal\":\"*\",\
                        \"Action\":\"execute-api:Invoke\",\"Resource\":\"execute-api:/*\"}]}";
        let other_path = "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\",\
                          \"Action\":\"execute-api:Invoke\",\
                          \"Resource\":\"execute-api:/s/GET/other\"}]}";
        let signed = Caller::Signed("arn:aws:iam::000000000000:user/emulator");
        // Rows differ along the effect the document states about this call and along the
        // method's own authorization — the two things the reading depends on.
        type Row<'a> = (&'a str, &'a str, Caller<'a>, bool);
        let rows: &[Row<'_>] = &[
            ("an allow, authorizing nobody", allow_all, Caller::Anonymous, true),
            ("an allow, IAM-authorized", allow_all, signed, true),
            ("a deny, authorizing nobody", deny_all, Caller::Anonymous, false),
            ("a deny, IAM-authorized", deny_all, signed, false),
            ("unstated, authorizing nobody", other_path, Caller::Anonymous, false),
            ("unstated, IAM-authorized", other_path, signed, true),
            ("not a policy at all", "not json", Caller::Anonymous, false),
        ];
        for (case, document, caller, allowed) in rows {
            let refusal = denial(document, &call(None, *caller));
            assert_eq!(
                refusal.is_none(),
                *allowed,
                "case {case}: expected allowed={allowed}, got {refusal:?}"
            );
        }
    }

    #[test]
    fn a_refusal_names_the_principal_the_action_and_the_method() {
        let refusal = denial("{\"Statement\":[]}", &call(None, Caller::Anonymous))
            .expect("a document stating nothing refuses a call the method authorizes nobody for");
        assert_eq!(
            refusal,
            "User: anonymous is not authorized to perform: execute-api:Invoke on resource: \
             arn:aws:execute-api:us-west-2:000000000000:abc1230000/s/GET/whoami",
            "case the refusal sentence: got {refusal:?}"
        );
    }

    #[test]
    fn invalidating_the_cached_response_is_authorized_by_its_own_action() {
        let grant = |action: &str| {
            format!(
                "{{\"Statement\":[{{\"Effect\":\"Allow\",\"Principal\":\"*\",\
                 \"Action\":\"{action}\",\"Resource\":\"execute-api:/*\"}}]}}"
            )
        };
        let deny_invalidation = grant("execute-api:InvalidateCache").replace("Allow", "Deny");
        let signed = Caller::Signed("arn:aws:iam::000000000000:user/emulator");
        // Rows differ along what the document states about the INVALIDATION action — the
        // action itself, another action of the same service, every action, nothing, an
        // explicit deny — and along the method's own authorization, which is what decides
        // an unstated document.
        type Row<'a> = (&'a str, Option<String>, Caller<'a>, bool);
        let rows: Vec<Row<'_>> = vec![
            (
                "the action itself, authorizing nobody",
                Some(grant(INVALIDATE_CACHE_ACTION)),
                Caller::Anonymous,
                true,
            ),
            (
                "the invoke alone, authorizing nobody",
                Some(grant(INVOKE_ACTION)),
                Caller::Anonymous,
                false,
            ),
            (
                "every action of the service, authorizing nobody",
                Some(grant("execute-api:*")),
                Caller::Anonymous,
                true,
            ),
            ("no document at all, authorizing nobody", None, Caller::Anonymous, false),
            ("no document at all, IAM-authorized", None, signed, true),
            (
                "an explicit deny, IAM-authorized",
                Some(deny_invalidation.clone()),
                signed,
                false,
            ),
            (
                "an explicit deny, authorizing nobody",
                Some(deny_invalidation),
                Caller::Anonymous,
                false,
            ),
        ];
        for (case, document, caller, authorized) in rows {
            let read = authorizes_invalidation(document.as_deref(), &call(None, caller));
            assert_eq!(
                read, authorized,
                "case {case}: expected authorized={authorized}, got {read}"
            );
        }
    }

    #[test]
    fn an_unauthorized_invalidation_names_its_own_action() {
        let refusal = invalidation_not_authorized(&call(None, Caller::Anonymous));
        assert_eq!(
            refusal,
            "User: anonymous is not authorized to perform: execute-api:InvalidateCache on \
             resource: arn:aws:execute-api:us-west-2:000000000000:abc1230000/s/GET/whoami",
            "case the refusal sentence: got {refusal:?}"
        );
    }
}
