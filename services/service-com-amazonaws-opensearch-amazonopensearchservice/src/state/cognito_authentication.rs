// SPDX-License-Identifier: CC0-1.0

//! Whether the Amazon Cognito authentication a request asks a domain's OpenSearch
//! Dashboards to be reached through is a configuration the service can actually
//! provision — the ONE statement of that rule, so the create path and the update path
//! judge a `CognitoOptions` container alike.
//!
//! `CognitoOptions` is "the parameters required to enable Cognito authentication for an
//! OpenSearch Service domain": the user pool that holds the Dashboards users, the
//! identity pool that exchanges a pool token for credentials, and the role "that allows
//! OpenSearch Service to configure your user pool and identity pool". Every member of
//! the container is modelled optional because the whole container is omittable and
//! because a container that enables nothing needs none of them — so what the model
//! cannot say, and this module does, is the CROSS-MEMBER rule: a container that enables
//! the feature has to carry all three, and the user pool it names has to be a pool that
//! exists AND that hosts the sign-in pages the Dashboards redirect is served by — a user
//! pool domain, prefix or custom. A configuration failing any of those describes a
//! sign-in path that can never work, so the request is refused with the operation's
//! `ValidationException` and nothing is stored.
//!
//! The pool is resolved in the user-pool service's own store, reached through the SAME
//! shared engine this service's handlers reach their own store through — the store
//! `cognito-idp:CreateUserPool` writes and `cognito-idp:DescribeUserPool` reports — the
//! way [`super::authorization`] resolves a caller in the IAM and STS stores. That store
//! belongs to the ENGINE rather than to a mounted service, so which services the
//! emulator happens to mount does not change what this check resolves: an engine this
//! service holds its own domains in holds the pools too. An engine that hands out no
//! user-pool store is one that hands out no store at all, which every handler here has
//! already answered its modelled server error for before reaching this check.

use ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state as user_pools;

use crate::ops::types::{CognitoOptions, ValidationException};

/// The members of `CognitoOptions` an enabled container has to carry, in the order the
/// model declares them — so a container that leaves out more than one is refused with
/// the same sentence however the request was built.
///
/// `UserPoolId` is "the Amazon Cognito user pool ID that you want OpenSearch Service to
/// use for OpenSearch Dashboards authentication", `IdentityPoolId` the identity pool for
/// the same, and `RoleArn` the role "that allows OpenSearch Service to configure your
/// user pool and identity pool" (`smithy_query(query="shape",shape="CognitoOptions")`):
/// each names one of the three things the feature is served by, so the feature cannot be
/// enabled without it.
const REQUIRED_MEMBERS: [&str; 3] = ["UserPoolId", "IdentityPoolId", "RoleArn"];

/// What the service answers a container enabling Cognito authentication against a user
/// pool that hosts no sign-in pages: the pool's own `Domain` is the thing missing, and
/// the sentence has no variable part — it names the member, not one request's pool.
const USER_POOL_WITHOUT_DOMAIN_MESSAGE: &str = "Domain needs to be specified for user pool";

/// Whether `options` describes Cognito authentication the service can provision, and the
/// refusal stating what is wrong with it otherwise.
///
/// `None` for a request that carries no container and for one whose container leaves the
/// feature disabled: `CognitoOptions$Enabled` is "whether to enable or disable Amazon
/// Cognito authentication for OpenSearch Dashboards", so a disabled container configures
/// no sign-in path and the members it leaves out are members nothing would have used.
pub fn cognito_options_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    options: Option<&CognitoOptions>,
) -> Option<ValidationException> {
    let options = options?;
    if !options.enabled.unwrap_or(false) {
        return None;
    }
    let supplied = [
        options.user_pool_id.as_deref(),
        options.identity_pool_id.as_deref(),
        options.role_arn.as_deref(),
    ];
    for (member, value) in REQUIRED_MEMBERS.iter().zip(supplied) {
        if value.is_none_or(str::is_empty) {
            return Some(ValidationException {
                message: Some(format!("{member} needs to be specified")),
            });
        }
    }
    let user_pool_id = options.user_pool_id.as_deref()?;
    // The pool is named by id, and an id that names no pool names nothing for the
    // Dashboards sign-in to authenticate against — so it is refused here rather than
    // stored as a working Cognito integration pointing at nothing. The sentence is the
    // one the user-pool service itself answers an unknown pool id with, so a caller is
    // told the same thing by whichever service it asked. The store comes from the
    // engine, not from a mounted service (see the module docs).
    let store = user_pools::service_state(engine)?;
    let pools = store.lock();
    let Some(pool) = pools.pool(user_pool_id) else {
        return Some(ValidationException {
            message: Some(format!("User pool {user_pool_id} does not exist.")),
        });
    };
    // A pool that exists is still not a sign-in path until it HOSTS one: Dashboards
    // sends the browser to the pool's managed login pages, which a pool serves only
    // once a user pool domain is created on it, and a pool holds at most one prefix
    // domain and at most one custom domain — either one hosts those pages, so either
    // satisfies this. A pool with neither can authenticate nobody, so the
    // configuration is refused and nothing is stored.
    if pool.domain.is_none() && pool.custom_domain.is_none() {
        return Some(ValidationException {
            message: Some(USER_POOL_WITHOUT_DOMAIN_MESSAGE.to_owned()),
        });
    }
    None
}
