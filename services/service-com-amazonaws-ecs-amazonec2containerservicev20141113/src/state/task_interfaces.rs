// SPDX-License-Identifier: CC0-1.0

//! The elastic network interface an `awsvpc` task is allocated: what it carries, and when.
//!
//! `RegisterTaskDefinition$networkMode`: "If the network mode is `awsvpc`, the task is
//! allocated an elastic network interface" — Amazon ECS creates that interface while the
//! task provisions and attaches it before the task runs. `Attachment$details` "for elastic
//! network interfaces ... includes the network interface ID, the MAC address, the subnet ID,
//! and the private IPv4 address": the interface carries all four from the moment it is
//! created, and the attachment's own status is what moves — `PRECREATED` while the task
//! provisions, `ATTACHED` once the task runs.
//!
//! The subnet comes from the launch (`AwsVpcConfiguration$subnets`); the other three are
//! the interface's own and are derived from the counter its attachment id was minted out
//! of, so each interface keeps its own values across every read and no two interfaces share
//! one.

use super::*;

use crate::ops::types::NetworkInterface;

/// `Attachment$status` of an attachment whose resource is in place. From the member's own
/// list of statuses ("`PRECREATED`, `CREATED`, `ATTACHING`, `ATTACHED`, `DETACHING`,
/// `DETACHED`, `DELETED`, and `FAILED`").
pub const ATTACHMENT_STATUS_ATTACHED: &str = "ATTACHED";

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

    /// Report the interface Amazon ECS allocated for this attachment as in place: the
    /// transition from "created" to "attached" that the task reaching `RUNNING` means has
    /// already happened.
    ///
    /// Only an attachment still in the state Amazon ECS created it in
    /// ([`ATTACHMENT_STATUS_PRECREATED`]) has its status moved: once an agent has spoken
    /// for it (`SubmitAttachmentStateChanges`: "Sent to acknowledge that an attachment
    /// changed states") the agent's word is the one the record holds. The interface's own
    /// details are reported either way ([`Self::report_allocated_interface`]), because they
    /// are the interface's and not the attachment's.
    ///
    /// Answers whether anything changed, which is what makes the task's `version` counter
    /// move ("Every time a task experiences a change ... the version counter is
    /// incremented").
    pub fn attach_allocated_interface(&mut self) -> bool {
        if self.attachment_type != ATTACHMENT_TYPE_ELASTIC_NETWORK_INTERFACE {
            return false;
        }
        let mut changed = self.report_allocated_interface();
        if self.status == ATTACHMENT_STATUS_PRECREATED {
            self.status = ATTACHMENT_STATUS_ATTACHED.to_string();
            changed = true;
        }
        changed
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
