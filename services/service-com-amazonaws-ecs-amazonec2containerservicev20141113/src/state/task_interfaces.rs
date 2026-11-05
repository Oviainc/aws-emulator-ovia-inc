// SPDX-License-Identifier: CC0-1.0

//! The elastic network interface an `awsvpc` task is allocated: what it carries, and when.
//!
//! `RegisterTaskDefinition$networkMode`: "If the network mode is `awsvpc`, the task is
//! allocated an elastic network interface" — Amazon ECS creates that interface while the
//! task provisions and attaches it before the task runs. `Attachment$details` "for elastic
//! network interfaces ... includes the network interface ID, the MAC address, the subnet ID,
//! and the private IPv4 address": the interface carries all four from the moment it is
//! created, and the attachment's own status is what moves — `PRECREATED` the moment Amazon
//! ECS creates it, `ATTACHING` while it is being attached, `ATTACHED` once it is in place.
//!
//! The subnet comes from the launch (`AwsVpcConfiguration$subnets`); the other three are
//! the interface's own and are derived from the counter its attachment id was minted out
//! of, so each interface keeps its own values across every read and no two interfaces share
//! one.
//!
//! The attachment's own STATUS is what the clock moves, one documented step at a time
//! ([`INTERFACE_ATTACH_LIFECYCLE`]), and the task it belongs to does not reach `PENDING`
//! before it is in place ([`placement_schedule`]) — which is also the instant the task is
//! `CONNECTED` ([`TaskRecord::report_connected`]).

use super::*;

use crate::ops::types::NetworkInterface;

/// `Attachment$status` of an attachment whose resource is in place. From the member's own
/// list of statuses ("`PRECREATED`, `CREATED`, `ATTACHING`, `ATTACHED`, `DETACHING`,
/// `DETACHED`, `DELETED`, and `FAILED`").
pub const ATTACHMENT_STATUS_ATTACHED: &str = "ATTACHED";

/// `Attachment$status` of an attachment Amazon ECS is in the middle of attaching to the
/// task it created it for, from the same list of statuses.
pub const ATTACHMENT_STATUS_ATTACHING: &str = "ATTACHING";

/// The statuses Amazon ECS takes an interface it is attaching THROUGH, in the order it
/// takes them: created for the task that is provisioning, being attached to it, and in
/// place. They are three of the values `Attachment$status` lists, and nothing here moves
/// an attachment to one of the others — a detach or a failure is not something this
/// emulator's clock owes anybody.
pub const INTERFACE_ATTACH_LIFECYCLE: [&str; 3] = [
    ATTACHMENT_STATUS_PRECREATED,
    ATTACHMENT_STATUS_ATTACHING,
    ATTACHMENT_STATUS_ATTACHED,
];

/// How long Amazon ECS takes over ONE step of [`INTERFACE_ATTACH_LIFECYCLE`].
///
/// EMULATOR-CHOSEN — AWS publishes no duration for creating and attaching a task's
/// interface — and deliberately much longer than one task transition
/// ([`TRANSITION_SPAN`]): the ECS agent ACKNOWLEDGES the status an attachment is in
/// (`SubmitAttachmentStateChanges`: "Sent to acknowledge that an attachment changed
/// states"), so every status the interface passes through has to last long enough for an
/// agent that polls the task to observe it and report on it, not merely to sit between two
/// reads of the record.
pub const INTERFACE_STEP_SPAN: Duration = Duration::from_secs(3);

/// What the clock owes a task the moment it is PLACED: the `lastStatus` it is placed in,
/// and when each of its next two transitions falls due.
///
/// A task placed onto capacity that already exists is `PENDING` with only its container
/// start owed ([`TRANSITION_SPAN`] away). A task that is allocated its own elastic network
/// interface is one step earlier in the lifecycle: Amazon ECS has to create and attach
/// that interface before the task can be launched, which is what `PROVISIONING` reports,
/// so such a task crosses `PROVISIONING` -> `PENDING` at the instant its interface is
/// attached ([`AttachmentRecord::attached_due_at`]) and `PENDING` -> `RUNNING` one
/// transition span after that. A task whose capacity still has to be ACQUIRED is
/// `PROVISIONING` with nothing owed at all: the placement itself is outstanding
/// ([`awaits_provisioned_capacity`]).
#[derive(Debug, Clone, Copy)]
pub struct PlacementSchedule {
    /// The `lastStatus` the task — and every container of it — is placed in.
    pub last_status: &'static str,
    /// When `PROVISIONING` -> `PENDING` falls due, for a task that is placed but whose
    /// networking is not in place yet.
    pub pending_due_at: Option<SystemTime>,
    /// When `PENDING` -> `RUNNING` falls due, for a task that is already `PENDING`.
    pub running_due_at: Option<SystemTime>,
}

/// The schedule a placement made at `now` leaves on the task, given whether its capacity
/// still has to be acquired and the attachments it was allocated. See
/// [`PlacementSchedule`].
pub fn placement_schedule(
    awaiting_capacity: bool,
    attachments: &[AttachmentRecord],
    now: SystemTime,
) -> PlacementSchedule {
    if awaiting_capacity {
        return PlacementSchedule {
            last_status: initial_task_status(true),
            pending_due_at: None,
            running_due_at: None,
        };
    }
    match attachments
        .iter()
        .filter_map(AttachmentRecord::attached_due_at)
        .max()
    {
        Some(attached_at) => PlacementSchedule {
            last_status: STATUS_PROVISIONING,
            pending_due_at: Some(attached_at),
            running_due_at: None,
        },
        None => PlacementSchedule {
            last_status: initial_task_status(false),
            pending_due_at: None,
            running_due_at: Some(now + TRANSITION_SPAN),
        },
    }
}

/// The `Attachment$details` key the interface's own id is reported under.
pub const ATTACHMENT_DETAIL_NETWORK_INTERFACE_ID: &str = "networkInterfaceId";

/// The `Attachment$details` key the interface's MAC address is reported under.
pub const ATTACHMENT_DETAIL_MAC_ADDRESS: &str = "macAddress";

/// The `Attachment$details` key the interface's private IPv4 address is reported under.
/// Amazon ECS spells this key with the `IP` of `IPv4` capitalised, unlike
/// `NetworkInterface$privateIpv4Address` on the container's own view.
pub const ATTACHMENT_DETAIL_PRIVATE_IPV4_ADDRESS: &str = "privateIPv4Address";

/// The id the `sequence`-th interface this store allocates carries: `eni-` and 17
/// hexadecimal digits, the form an elastic network interface id takes. The counter starts
/// at one, so the id is never the null id.
pub fn allocated_interface_id(sequence: u64) -> String {
    format!("eni-{sequence:017x}")
}

/// The MAC address the `sequence`-th interface carries: six octets whose first is `0a`,
/// which makes the address locally administered and unicast, and whose remaining five spell
/// the counter — so one interface's address is its own.
pub fn allocated_interface_mac_address(sequence: u64) -> String {
    let octet = |shift: u32| (sequence >> shift) & 0xff;
    format!(
        "0a:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        octet(32),
        octet(24),
        octet(16),
        octet(8),
        octet(0)
    )
}

/// The private IPv4 address the `sequence`-th interface carries.
///
/// Drawn from `172.31.0.0/16`, the CIDR AWS gives a default VPC (Amazon VPC User Guide,
/// "Default VPCs"), skipping the first four addresses of each `/24` — AWS reserves them —
/// and the broadcast address, so every address handed out is one a network interface may
/// actually take.
pub fn allocated_interface_private_ipv4_address(sequence: u64) -> String {
    format!("172.31.{}.{}", (sequence / 251) % 256, 4 + sequence % 251)
}

impl AttachmentRecord {
    /// Report, on this attachment, the three details that belong to the interface itself:
    /// its id, its MAC address and its private IPv4 address.
    ///
    /// Amazon ECS creates the interface before the task runs, so they are reported from the
    /// instant the attachment exists, beside the subnet the launch named. Each is written
    /// once — a detail already reported is left as it is — and nothing but an elastic
    /// network interface gets any of them.
    ///
    /// Answers whether anything changed.
    pub fn report_allocated_interface(&mut self) -> bool {
        if self.attachment_type != ATTACHMENT_TYPE_ELASTIC_NETWORK_INTERFACE {
            return false;
        }
        let mut changed = false;
        for (name, value) in [
            (
                ATTACHMENT_DETAIL_NETWORK_INTERFACE_ID,
                allocated_interface_id(self.sequence),
            ),
            (
                ATTACHMENT_DETAIL_MAC_ADDRESS,
                allocated_interface_mac_address(self.sequence),
            ),
            (
                ATTACHMENT_DETAIL_PRIVATE_IPV4_ADDRESS,
                allocated_interface_private_ipv4_address(self.sequence),
            ),
        ] {
            if self
                .details
                .iter()
                .any(|detail| detail.name.as_deref() == Some(name))
            {
                continue;
            }
            self.details.push(KeyValuePair {
                name: Some(name.to_string()),
                value: Some(value),
            });
            changed = true;
        }
        changed
    }

    /// Report the interface Amazon ECS allocated for this attachment as in place: the end
    /// of [`INTERFACE_ATTACH_LIFECYCLE`], which the task reaching `RUNNING` means has
    /// already happened.
    ///
    /// Only an attachment the clock is still taking through that progression has its status
    /// moved: once an agent has spoken for it with a status off the progression
    /// (`SubmitAttachmentStateChanges`: "Sent to acknowledge that an attachment changed
    /// states") the agent's word is the one the record holds. The interface's own details
    /// are reported either way ([`Self::report_allocated_interface`]), because they are the
    /// interface's and not the attachment's.
    ///
    /// Answers whether anything changed, which is what makes the task's `version` counter
    /// move ("Every time a task experiences a change ... the version counter is
    /// incremented").
    pub fn attach_allocated_interface(&mut self) -> bool {
        if !self.is_elastic_network_interface() {
            return false;
        }
        let mut changed = self.report_allocated_interface();
        if INTERFACE_ATTACH_LIFECYCLE
            .iter()
            .any(|status| *status == self.status)
            && self.status != ATTACHMENT_STATUS_ATTACHED
        {
            self.status = ATTACHMENT_STATUS_ATTACHED.to_string();
            self.next_status_due_at = None;
            changed = true;
        }
        changed
    }

    /// Whether this attachment is the elastic network interface an `awsvpc` task is
    /// allocated, rather than some other kind of attachment a task can carry.
    pub fn is_elastic_network_interface(&self) -> bool {
        self.attachment_type == ATTACHMENT_TYPE_ELASTIC_NETWORK_INTERFACE
    }

    /// Whether the resource this attachment stands for is in place.
    pub fn is_attached(&self) -> bool {
        self.status == ATTACHMENT_STATUS_ATTACHED
    }

    /// The status [`INTERFACE_ATTACH_LIFECYCLE`] takes this attachment to next, or `None`
    /// when the clock owes it nothing more: it is already attached, or an agent has
    /// reported a status that is not on that progression at all.
    fn next_attach_status(&self) -> Option<&'static str> {
        let reached = INTERFACE_ATTACH_LIFECYCLE
            .iter()
            .position(|status| *status == self.status)?;
        INTERFACE_ATTACH_LIFECYCLE.get(reached + 1).copied()
    }

    /// The instant this attachment is expected to report [`ATTACHMENT_STATUS_ATTACHED`],
    /// from where its progression has got to, and `None` once nothing more is owed — the
    /// instant a task that cannot be launched before its interface is in place is placed
    /// at ([`placement_schedule`]).
    pub fn attached_due_at(&self) -> Option<SystemTime> {
        let due = self.next_status_due_at?;
        let reached = INTERFACE_ATTACH_LIFECYCLE
            .iter()
            .position(|status| *status == self.status)?;
        // The next step falls due at `due`, and every step after it one span further on.
        let steps_left = INTERFACE_ATTACH_LIFECYCLE.len() - 1 - reached;
        let after_next = steps_left.checked_sub(1)?;
        Some(due + INTERFACE_STEP_SPAN * after_next as u32)
    }

    /// Materialise every step of this attachment's own progression that the clock has
    /// already made due as of `now`, and answer the instant of the last one it took (or
    /// `None` when nothing was due).
    ///
    /// The progression continues from whatever status the record HOLDS, so an agent that
    /// acknowledged a status takes the attachment to the step it reported and Amazon ECS
    /// carries on from there; a status that is not on the progression at all ends it.
    pub fn advance_interface(&mut self, now: SystemTime) -> Option<SystemTime> {
        let mut last_step = None;
        while let Some(due) = self.next_status_due_at.filter(|due| *due <= now) {
            match self.next_attach_status() {
                Some(next) => {
                    self.status = next.to_string();
                    last_step = Some(due);
                    self.next_status_due_at = self
                        .next_attach_status()
                        .map(|_| due + INTERFACE_STEP_SPAN);
                }
                // Nothing further is owed: the progression is over, whoever ended it.
                None => self.next_status_due_at = None,
            }
        }
        last_step
    }

    /// The private IPv4 address this attachment reports, or `None` while the interface has
    /// not been attached yet — read off the stored details rather than recomputed, so it is
    /// the value a caller of `DescribeTasks` sees.
    pub fn private_ipv4_address(&self) -> Option<&str> {
        self.details
            .iter()
            .find(|detail| {
                detail.name.as_deref() == Some(ATTACHMENT_DETAIL_PRIVATE_IPV4_ADDRESS)
            })
            .and_then(|detail| detail.value.as_deref())
    }

    /// The `NetworkInterface` a container of the task holding this attachment reports for
    /// it, or `None` when this attachment is not an interface — a container of an `awsvpc`
    /// task reports the interface it is reached on.
    pub fn container_network_interface(&self) -> Option<NetworkInterface> {
        if self.attachment_type != ATTACHMENT_TYPE_ELASTIC_NETWORK_INTERFACE {
            return None;
        }
        Some(NetworkInterface {
            attachment_id: Some(self.id.clone()),
            private_ipv4_address: self.private_ipv4_address().map(str::to_string),
            // No IPv6 address: nothing here allocates one, and reporting an address the
            // interface does not hold would be reporting a reachable address that is not.
            ipv6_address: None,
        })
    }
}
