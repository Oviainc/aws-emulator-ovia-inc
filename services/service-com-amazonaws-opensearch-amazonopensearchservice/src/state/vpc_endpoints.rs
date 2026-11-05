// SPDX-License-Identifier: CC0-1.0

//! VPC endpoint access and the service-managed VPC endpoints themselves, split out of
//! [`crate::state`] unchanged: an authorization is one principal's access to ONE domain
//! through an interface endpoint, and an endpoint is the proxy a caller in ANOTHER VPC
//! reaches one domain through.

use super::*;

/// `AuthorizedPrincipal$PrincipalType` for an account grantee: the authorized principal
/// is an Amazon Web Services account, named by its account id. The `PrincipalType` enum
/// declares this kind as `AWS_ACCOUNT`, and that is what the service answers for it.
pub const PRINCIPAL_TYPE_AWS_ACCOUNT: &str = "AWS_ACCOUNT";

/// `AuthorizedPrincipal$PrincipalType` for a SERVICE grantee: the authorized principal
/// is a service, named by its service principal.
///
/// The service answers this kind in words rather than as the `PrincipalType` enum
/// declares it (`AWS_SERVICE`): what a grant and a listing report for a service grantee
/// is `AWS Service`, which is what a client switching on the value it read back — or
/// round-tripping it into a grant list — has to match.
pub const PRINCIPAL_TYPE_AWS_SERVICE: &str = "AWS Service";

/// How many principals ONE domain's VPC endpoint access set holds before
/// `AuthorizeVpcEndpointAccess` refuses with the modelled `LimitExceededException`.
///
/// Like [`DATA_SOURCE_QUOTA`] this is the emulator's OWN bound on the store — it
/// models neither Service Quotas nor an account's adjustments — and it is what stops
/// a caller looping with fresh principals from growing the state without end: every
/// record it counts is released when `RevokeVpcEndpointAccess` releases the one
/// principal's access, or when `DeleteDomain` releases the domain that holds the set.
/// Because the per-domain set IS what `ListVpcEndpointAccess` answers with, and that
/// set is held on the domain, with [`DOMAIN_QUOTA`] it also bounds the whole
/// authorization store.
pub const VPC_ENDPOINT_ACCESS_QUOTA: usize = 50;

/// The principal a VPC endpoint access call names: what kind of principal it is, and the
/// identity that names it.
#[derive(Debug, Clone)]
pub struct RequestedPrincipal {
    /// `AuthorizedPrincipal$PrincipalType` as the service answers it —
    /// [`PRINCIPAL_TYPE_AWS_ACCOUNT`] or [`PRINCIPAL_TYPE_AWS_SERVICE`].
    pub principal_type: &'static str,
    /// `AuthorizedPrincipal$Principal`, and the key a domain's set holds the record
    /// under.
    pub id: String,
}

/// The principal an `Account`/`Service` pair names, or the refusal for a request that
/// names no principal or more than one.
///
/// An authorization is ONE principal's access to the domain and the grant's answer
/// carries exactly one `AuthorizedPrincipal`, whose `PrincipalType` says which of the two
/// kinds it is — so a request names EXACTLY one of them. Both members are optional in the
/// model and nothing in it relates them, so neither combination is refused at the request
/// boundary: it is the operation's own modelled `ValidationException` that refuses them,
/// with 400.
///
/// `AuthorizeVpcEndpointAccess` and `RevokeVpcEndpointAccess` take that same pair and
/// address the same per-domain set with it, so the rule reading it lives here once and
/// the two cannot disagree about which principal a request named.
pub fn requested_principal(
    account: Option<String>,
    service: Option<String>,
) -> Result<RequestedPrincipal, ValidationException> {
    match (account, service) {
        (Some(account), None) => Ok(RequestedPrincipal {
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            id: account,
        }),
        (None, Some(service)) => Ok(RequestedPrincipal {
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            id: service,
        }),
        (None, None) => Err(ValidationException {
            message: Some(
                "either Account or Service is required: it names the principal whose \
                 access to the domain through an interface VPC endpoint this call changes"
                    .to_string(),
            ),
        }),
        (Some(_), Some(_)) => Err(ValidationException {
            message: Some(
                "Account and Service are mutually exclusive: one call changes the access \
                 of one principal"
                    .to_string(),
            ),
        }),
    }
}

/// Where an authorization listing resumes from, or `None` where `next_token` names
/// no place in `ordered`.
///
/// `ListVpcEndpointAccess` reports a domain's WHOLE authorization set in one answer —
/// the set is bounded by [`VPC_ENDPOINT_ACCESS_QUOTA`], so no answer leaves anything
/// unreported — and therefore hands out no continuation token of its own. This is the
/// one place that decides what a token sent to it means: the EMPTY token is the start
/// of the set, as naming none is, and a token naming a principal the set holds is the
/// place the answer starts at. Anything else names no place, which the caller words as
/// the modelled refusal.
pub fn vpc_endpoint_access_start<'a, I>(ordered: I, next_token: Option<&str>) -> Option<usize>
where
    I: IntoIterator<Item = &'a String>,
{
    match next_token.filter(|token| !token.is_empty()) {
        None => Some(0),
        Some(token) => ordered
            .into_iter()
            .position(|principal| principal == token),
    }
}

// ---------------------------------------------------------------------------
// Service-managed VPC endpoints
//
// An endpoint is the proxy a caller in ANOTHER VPC reaches one domain through:
// `CreateVpcEndpoint` mints one, `DescribeVpcEndpoints` reports the ones a
// request names, `ListVpcEndpoints` reports the account's set and
// `DeleteVpcEndpoint` releases one. [`VpcEndpointRecord`] below is the ONE copy
// all of them read and write, plus the two projections the model asks for
// (`VpcEndpoint` and `VpcEndpointSummary`), so no two of them can report the
// same endpoint differently.
//
// The endpoint is held BESIDE the domains rather than on the domain it serves:
// it is addressed by its own id, by callers that name no domain at all
// (`DescribeVpcEndpoints`, `ListVpcEndpoints`), and it names the domain by ARN
// the way the model does.
// ---------------------------------------------------------------------------

/// `VpcEndpointStatus`'s `CREATING` value: the endpoint has been asked for and is
/// being provisioned.
pub const VPC_ENDPOINT_STATUS_CREATING: &str = "CREATING";

/// `VpcEndpointStatus`'s `ACTIVE` value: the endpoint is up and can be connected
/// through.
pub const VPC_ENDPOINT_STATUS_ACTIVE: &str = "ACTIVE";

/// `VpcEndpointStatus`'s `UPDATING` value: a change to the endpoint's subnets or
/// security groups has been asked for and is being applied.
pub const VPC_ENDPOINT_STATUS_UPDATING: &str = "UPDATING";

/// `VpcEndpointStatus`'s `DELETING` value: the endpoint has been released and is
/// being torn down.
pub const VPC_ENDPOINT_STATUS_DELETING: &str = "DELETING";

/// The words the service names a change in flight on a service-managed VPC endpoint
/// by, in the conflict refusal a second writer gets: the lifecycle verb for each
/// transient `VpcEndpointStatus`, lower-case as the refusal states it.
pub const VPC_ENDPOINT_CHANGE_CREATE: &str = "create";
/// The verb for [`VPC_ENDPOINT_STATUS_UPDATING`]. See [`VPC_ENDPOINT_CHANGE_CREATE`].
pub const VPC_ENDPOINT_CHANGE_UPDATE: &str = "update";
/// The verb for [`VPC_ENDPOINT_STATUS_DELETING`]. See [`VPC_ENDPOINT_CHANGE_CREATE`].
pub const VPC_ENDPOINT_CHANGE_DELETE: &str = "delete";

/// `VpcEndpointErrorCode`'s `ENDPOINT_NOT_FOUND` value — the code
/// `DescribeVpcEndpoints` reports IN ITS PAYLOAD for an id that names no endpoint,
/// rather than refusing the whole call: that read reports on a LIST of ids, so one
/// id nothing holds is an error about that id and not about the request.
pub const VPC_ENDPOINT_ERROR_ENDPOINT_NOT_FOUND: &str = "ENDPOINT_NOT_FOUND";

/// How many service-managed VPC endpoints this account holds before
/// `CreateVpcEndpoint` refuses with the modelled `LimitExceededException`.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — it models
/// neither Service Quotas nor an account's adjustments — and it is what stops a
/// caller looping from growing the state without end: every record it counts is
/// released by deleting the endpoint. Because the stored set IS what
/// `ListVpcEndpoints` reports, it also bounds that listing.
pub const VPC_ENDPOINT_QUOTA: usize = 50;

/// An endpoint listing's answer: the summaries it carries, or `None` where
/// `next_token` names no place in `ordered`.
///
/// BOTH operations that list endpoints go through this — the account's whole set and
/// one domain's — so they cannot drift on what an answer holds or on what a token
/// sent to them means. `ordered` is the set in the order the caller reports it, which
/// is the store's own key order, and is what makes a token a PLACE in the set rather
/// than an offset a create or a release would have shifted.
///
/// The answer runs to the END of the set: the stored set is bounded by
/// [`VPC_ENDPOINT_QUOTA`], so one answer always reports everything the caller has not
/// been shown, and neither listing has a continuation token to hand out. The EMPTY
/// token therefore means the start of the set, exactly as naming no token does, and
/// `None` — a token naming no endpoint of the set — is left for the caller to word,
/// which is the modelled `ValidationException` both listings refuse it with
/// ([`invalid_next_token`]).
pub fn vpc_endpoint_listing(
    ordered: &[&VpcEndpointRecord],
    next_token: Option<&str>,
) -> Option<Vec<VpcEndpointSummary>> {
    let start = match next_token.filter(|token| !token.is_empty()) {
        None => 0,
        Some(token) => ordered
            .iter()
            .position(|endpoint| endpoint.vpc_endpoint_id == token)?,
    };
    Some(
        ordered
            .iter()
            .skip(start)
            .map(|endpoint| endpoint.to_summary())
            .collect(),
    )
}

/// One service-managed VPC endpoint of this account, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct VpcEndpointRecord {
    /// `VpcEndpoint$VpcEndpointId`: unique per endpoint, so it IS the key the store
    /// holds the record under.
    pub vpc_endpoint_id: String,
    /// `VpcEndpoint$VpcEndpointOwner`: "the creator of the endpoint", which is the
    /// account this emulator serves.
    pub owner: String,
    /// `VpcEndpoint$DomainArn`: the domain the endpoint connects to, as the request
    /// named it — and it named a domain this account holds, which is what
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::vpc_endpoint_domain`]
    /// resolved before the record was made.
    pub domain_arn: String,
    /// `VpcEndpoint$VpcOptions`: the subnets and security groups the endpoint was
    /// created with, stored as the modelled shape the read paths report rather than
    /// as a second mirror of its members.
    pub vpc_options: VPCDerivedInfo,
    /// Where the endpoint is in its lifecycle: one of `VpcEndpointStatus`'s values.
    pub status: String,
    /// The status move the clock owes this endpoint, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// `VpcEndpoint$Endpoint`: "the connection endpoint ID for connecting to the
    /// domain".
    pub endpoint: String,
    /// The request's `ClientToken`, absent where it carried none: "unique,
    /// case-sensitive identifier to ensure idempotency of the request", so a repeat
    /// of the same create is answered with THIS record rather than minting a second
    /// endpoint.
    pub client_token: Option<String>,
}

impl VpcEndpointRecord {
    /// Materialise the status move the clock owes this endpoint, if `now` is past its
    /// due instant: an endpoint that was being provisioned becomes one that is up.
    ///
    /// This is the only write a reporting operation makes, and it writes only what was
    /// already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
    }

    /// Whether a change to this endpoint has not landed yet, which is what makes a
    /// second writer's call conflict with the one already in flight.
    ///
    /// Read AFTER [`Self::settle`], so a move that has merely fallen due does not read
    /// as one still in flight. The same shape as [`PackageAssociation::in_flight`],
    /// because it is the same question about a resource this control plane is still
    /// changing.
    pub fn in_flight(&self) -> bool {
        self.owed.is_some()
    }

    /// How the service names the change that has not landed on this endpoint yet, or
    /// `None` where nothing is in flight.
    ///
    /// The endpoint's own status IS that change — it is the status the service leaves an
    /// endpoint in while the change is applied — so the word comes from the status
    /// rather than from a second record of what the last writer asked for, and from the
    /// stored state rather than from the request now being refused: what the refused
    /// caller has to wait for is the change ALREADY under way, whichever call started it.
    pub fn change_in_flight(&self) -> Option<&'static str> {
        if !self.in_flight() {
            return None;
        }
        Some(match self.status.as_str() {
            VPC_ENDPOINT_STATUS_CREATING => VPC_ENDPOINT_CHANGE_CREATE,
            VPC_ENDPOINT_STATUS_UPDATING => VPC_ENDPOINT_CHANGE_UPDATE,
            // The release takes the record out of the store and leaves no move owed on
            // it, so no endpoint this mapping is asked about is being torn down today;
            // the arm is here because the mapping is about the STATUS and is answered
            // the same way whichever status the record carries.
            VPC_ENDPOINT_STATUS_DELETING => VPC_ENDPOINT_CHANGE_DELETE,
            // A move owed from any OTHER status: the three above are every status this
            // store leaves an endpoint in while a change is applied, so this is the
            // surprising case, and a modification of the endpoint is what it reports.
            _ => VPC_ENDPOINT_CHANGE_UPDATE,
        })
    }

    /// Apply the subnets and security groups `options` names to this endpoint and
    /// leave it in the status that change starts.
    ///
    /// `VpcOptions` is "the security groups and/or subnets to add, remove, or modify":
    /// a member the request CARRIES replaces what the endpoint had, and a member it
    /// leaves out names nothing to change, so it keeps the value the endpoint already
    /// carries rather than being cleared.
    ///
    /// `placements` is what EC2 reports for the subnets the request names
    /// ([`crate::state::vpc_network`]), which is where the two DERIVED members of
    /// `VPCDerivedInfo` come from: the zones follow the new subnets, and `VPCId` is left
    /// exactly as it was — it is derived at creation and no call can move an endpoint
    /// into another VPC. A placement EC2 resolved nothing for leaves the zones as they
    /// were rather than clearing them, because nothing was learned about the new ones.
    ///
    /// The change is not in place the instant the call returns: the endpoint is being
    /// modified, and the clock owes it the status that modification leaves it in once
    /// the span has passed — which is what a second writer arriving meanwhile sees
    /// ([`Self::in_flight`]).
    pub fn begin_update(
        &mut self,
        options: VPCOptions,
        placements: &[crate::state::vpc_network::SubnetPlacement],
        now: SystemTime,
    ) {
        if options.subnet_ids.is_some() {
            self.vpc_options.subnet_ids = options.subnet_ids;
            if let Some(zones) = crate::state::vpc_network::placed_zones(placements) {
                self.vpc_options.availability_zones = Some(zones);
            }
        }
        if options.security_group_ids.is_some() {
            self.vpc_options.security_group_ids = options.security_group_ids;
        }
        if options.egress_enabled.is_some() {
            self.vpc_options.egress_enabled = options.egress_enabled;
        }
        self.status = VPC_ENDPOINT_STATUS_UPDATING.to_string();
        self.owed = Some(OwedStatus {
            status: VPC_ENDPOINT_STATUS_ACTIVE.to_string(),
            message: None,
            due: now + PROVISIONING,
        });
    }

    /// This record as `VpcEndpoint` — the shape `CreateVpcEndpoint` answers with and
    /// `DescribeVpcEndpoints` reports, so a create's answer and a later read of the
    /// endpoint cannot disagree.
    pub fn to_endpoint(&self) -> VpcEndpoint {
        VpcEndpoint {
            vpc_endpoint_id: Some(self.vpc_endpoint_id.clone()),
            vpc_endpoint_owner: Some(self.owner.clone()),
            domain_arn: Some(self.domain_arn.clone()),
            vpc_options: Some(self.vpc_options.clone()),
            status: Some(self.status.clone()),
            endpoint: Some(self.endpoint.clone()),
        }
    }

    /// This record as `VpcEndpointSummary` — the shape `ListVpcEndpoints` pages
    /// through and `DeleteVpcEndpoint` answers with: the same record as
    /// [`Self::to_endpoint`] without the subnets and the connection endpoint, which
    /// the model's summary declares no member for.
    pub fn to_summary(&self) -> VpcEndpointSummary {
        VpcEndpointSummary {
            vpc_endpoint_id: Some(self.vpc_endpoint_id.clone()),
            vpc_endpoint_owner: Some(self.owner.clone()),
            domain_arn: Some(self.domain_arn.clone()),
            status: Some(self.status.clone()),
        }
    }
}

/// The `ValidationException` payload for a `CreateVpcEndpoint` whose `DomainArn`
/// names no domain this account holds, or names one no endpoint can be created for.
///
/// The service states these as ONE refusal, and deliberately: which of them holds is
/// something the caller may not be entitled to learn, so the sentence names all the
/// reasons rather than the one that applied. It carries no value of the request for
/// the same reason.
pub fn vpc_endpoint_domain_unavailable() -> ValidationException {
    ValidationException {
        message: Some(
            "Either the domain doesn't exist, it doesn't support creation of VPC \
             endpoints, or you don't have permission to create a VPC endpoint for the \
             domain."
                .to_string(),
        ),
    }
}

/// The `ValidationException` payload for a change that would move a service-managed VPC
/// endpoint into another VPC: a subnet the request names belongs to a VPC other than the
/// one the endpoint was created in.
///
/// An endpoint's `VPCDerivedInfo$VPCId` is derived from the subnets it was created in and
/// is not a member any call can set, so the VPC an endpoint is in is fixed at creation:
/// the subnets a change names have to be subnets of that VPC. The sentence names the
/// subnet the request asked for and the VPC the endpoint is in — the one read from this
/// service's own state — so the caller can tell which of the subnets it listed is the
/// wrong one.
pub fn subnet_outside_endpoint_vpc(subnet_id: &str, vpc_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "The subnet {subnet_id} doesn't belong to VPC {vpc_id}. The subnets of a VPC \
             endpoint must all belong to the VPC the endpoint was created in."
        )),
    }
}

/// service-managed VPC endpoint: fixed text carrying no value of the request, because
/// the entry beside it already names the id it is about.
pub const ENDPOINT_DOES_NOT_EXIST: &str = "Endpoint does not exist";

/// The sentence a call that ADDRESSES one endpoint refuses a missing id with.
///
/// Worded differently from [`ENDPOINT_DOES_NOT_EXIST`], and deliberately: a caller
/// addressing an endpoint may not be entitled to learn which of the two reasons holds,
/// so the refusal names both rather than the one that applied — the same reticence
/// [`vpc_endpoint_domain_unavailable`] shows about a domain.
pub const ENDPOINT_INACCESSIBLE: &str =
    "The VPC endpoint doesn't exist, or you don't have access to it.";

/// The `ResourceNotFoundException` payload for an id that names no endpoint of this
/// account — the endpoint mirror of [`domain_not_found`].
///
/// This is what a call ADDRESSING one endpoint (releasing it) answers; a read that
/// names a LIST of ids reports the same condition per id in its payload instead
/// ([`vpc_endpoint_missing`]).
pub fn vpc_endpoint_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(ENDPOINT_INACCESSIBLE.to_string()),
    }
}

/// The `ConflictException` payload for a call that would change a service-managed VPC
/// endpoint whose last change has not landed yet.
///
/// The service applies one change to an endpoint at a time, so a writer that arrives
/// while another is still in flight is refused rather than overtaking it — the same
/// rule [`concurrent_package_operation`] states for a domain's package associations.
/// The sentence states the change ALREADY in progress on the endpoint
/// ([`VpcEndpointRecord::change_in_flight`]) and carries no value of the request: what
/// the refused caller has to wait for is that change, and the endpoint is the one it
/// already named.
pub fn vpc_endpoint_being_modified(change_in_flight: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "A {change_in_flight} operation is already in progress for this VPC endpoint."
        )),
    }
}

/// The `ValidationException` payload for a read that names more endpoint ids than this
/// account can hold endpoints: the answer carries one entry per id the request names,
/// so the request would otherwise choose the size of the response.
pub fn too_many_vpc_endpoint_ids(named: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "VpcEndpointIds names {named} endpoints; at most {VPC_ENDPOINT_QUOTA} can be \
             described in one call."
        )),
    }
}

/// The `VpcEndpointError` `DescribeVpcEndpoints` reports for one id that names no
/// endpoint: the id it was asked about, the modelled code for that condition and the
/// service's own sentence for it.
pub fn vpc_endpoint_missing(vpc_endpoint_id: &str) -> VpcEndpointError {
    VpcEndpointError {
        vpc_endpoint_id: Some(vpc_endpoint_id.to_string()),
        error_code: Some(VPC_ENDPOINT_ERROR_ENDPOINT_NOT_FOUND.to_string()),
        error_message: Some(ENDPOINT_DOES_NOT_EXIST.to_string()),
    }
}
