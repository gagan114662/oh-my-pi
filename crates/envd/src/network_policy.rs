//! Destination rules shared by command sandbox and native HTTP egress.

use std::net::IpAddr;

use crate::policy::{SandboxDomainRule, SandboxNetworkPolicy};

impl SandboxNetworkPolicy {
	/// Evaluates host and port together; domain-specific grants are never
	/// widened into a cross product of independently allowed hosts and ports.
	pub(crate) fn allows_destination(&self, host: &str, port: u16) -> bool {
		if self.mode == "open" {
			return true;
		}
		self.mode == "proxy"
			&& !host.is_empty()
			&& port != 0
			&& (self.allow_ports.is_empty() || self.allow_ports.contains(&port))
			&& !self
				.deny_domains
				.iter()
				.any(|rule| rule_matches(rule, host, port))
			&& self
				.allow_domains
				.iter()
				.any(|rule| rule_matches(rule, host, port))
	}

	pub(crate) fn allows_address(&self, ip: IpAddr) -> bool {
		self.mode == "open" || self.mode == "proxy" && authorized_address(ip, self.allow_localhost)
	}
}

fn rule_matches(rule: &SandboxDomainRule, host: &str, port: u16) -> bool {
	(rule.ports.is_empty() || rule.ports.contains(&port)) && domain_matches(&rule.domain, host)
}

pub(crate) fn domain_matches(rule: &str, host: &str) -> bool {
	let rule = rule.trim().trim_end_matches('.');
	if let Some(suffix) = rule.strip_prefix("*.") {
		let suffix = suffix.to_ascii_lowercase();
		host.len() > suffix.len()
			&& host.ends_with(&suffix)
			&& host.as_bytes().get(host.len() - suffix.len() - 1) == Some(&b'.')
	} else {
		rule.eq_ignore_ascii_case(host)
	}
}

pub(crate) fn authorized_address(ip: IpAddr, allow_localhost: bool) -> bool {
	globally_routable(ip) || allow_localhost && ip.is_loopback()
}

pub(crate) fn globally_routable(ip: IpAddr) -> bool {
	match ip {
		IpAddr::V4(value) => {
			!(value.is_private()
				|| value.is_loopback()
				|| value.is_link_local()
				|| value.is_multicast()
				|| value.is_unspecified()
				|| value.is_broadcast()
				|| value.octets()[0] == 0
				|| matches!(
					value.octets(),
					[100, 64..=127, _, _]
						| [192, 0, 0, _]
						| [192, 0, 2, _]
						| [198, 18..=19, _, _]
						| [198, 51, 100, _]
						| [203, 0, 113, _]
						| [240..=255, _, _, _]
						| [168, 63, 129, 16]
				))
		},
		IpAddr::V6(value) => {
			if let Some(mapped) = value.to_ipv4_mapped() {
				return globally_routable(IpAddr::V4(mapped));
			}
			!(value.is_loopback()
				|| value.is_multicast()
				|| value.is_unspecified()
				|| value.is_unique_local()
				|| value.is_unicast_link_local())
		},
	}
}

#[cfg(test)]
mod tests {
	use omp_core::Str;

	use super::*;
	#[test]
	fn domain_ports_remain_paired_and_denials_are_port_specific() {
		let mut policy = SandboxNetworkPolicy {
			allow_ports: Vec::new(),
			allow_domains: vec![
				SandboxDomainRule { domain: Str::from("a.test"), ports: vec![443] },
				SandboxDomainRule { domain: Str::from("b.test"), ports: vec![8443] },
			],
			..Default::default()
		};
		assert!(policy.allows_destination("a.test", 443));
		assert!(policy.allows_destination("b.test", 8443));
		assert!(!policy.allows_destination("a.test", 8443));
		assert!(!policy.allows_destination("b.test", 443));
		policy
			.deny_domains
			.push(SandboxDomainRule { domain: Str::from("a.test"), ports: vec![443] });
		assert!(!policy.allows_destination("a.test", 443));
		assert!(policy.allows_destination("b.test", 8443));
	}
}
