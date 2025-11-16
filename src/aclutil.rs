use std::fs;
use std::path::Path;
use serde::{Deserialize, Deserializer, Serialize};
use serde::de::{self, Visitor};
use tracing::{debug, info};
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use regex::RegexBuilder;
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use hcl::from_str;
use anyhow::Result;

use crate::util;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACLFile {
    pub acl: Vec<ACLRule>,
}

impl ACLFile {
    /// Evaluate whether a connection should be allowed or denied.
    /// 
    /// # Arguments
    /// * `source_ip` - Source IP address in string format (IPv4 or IPv6)
    /// * `target` - Target in "host:port" format (e.g., "example.com:80" or "192.168.1.1:443")
    /// 
    /// # Returns
    /// * `ACLAction::Allow` if a matching rule allows it
    /// * `ACLAction::Deny` if a matching rule denies it, or if no rule matches (default deny)
    pub fn evaluate(&self, source_ip: &str, target: &str) -> ACLAction {
        // Parse target into host and port
        let (target_host, target_port) = match Self::parse_target(target) {
            Ok((host, port)) => (host, port),
            Err(_) => {
                info!("denied because of invalid target (not host and port): {}", target);
                return ACLAction::Deny
            }, // Invalid target format, deny by default
        };

        debug!("evaluating ACL, target host: {} target port: {}", target_host, target_port);
        // Parse source IP
        let source_ip_addr = match IpAddr::from_str(source_ip) {
            Ok(addr) => addr,
            Err(_) => {
                info!("denied because of invalid source IP: {}", source_ip);
                return ACLAction::Deny
            }, // Invalid IP, deny by default
        };

        // Go through rules in sorted order (already sorted by priority high->low, then index low->high)
        for rule in &self.acl {
            if Self::matches_rule(rule, &source_ip_addr, &target_host, target_port) {
                let index = rule.index;
                let priority = rule.priority;
                let hosts = &rule.to.hosts;
                let ports = &rule.to.ports;
                debug!("ACL rule matched, index: {} priority: {} action: {:?} hosts: {:?} ports: {:?}", index, priority, rule.action, hosts, ports);
                return rule.action.clone();
            }
        }

        // No rule matched, default to deny
        info!("denied because no rule matched src ip: {} target: {}", source_ip, target);
        ACLAction::Deny
    }

    fn parse_target(target: &str) -> Result<(String, u16), Box<dyn std::error::Error>> {
        let parts: Vec<&str> = target.rsplitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!("Invalid target format: expected 'host:port'").into());
        }
        let port = parts[0].parse::<u16>()
            .map_err(|e| anyhow::anyhow!("Invalid port: {}", e))?;
        let host = parts[1].to_string();
        Ok((host, port))
    }

    fn matches_rule(rule: &ACLRule, source_ip: &IpAddr, target_host: &str, target_port: u16) -> bool {
        // Check if source IP matches any of the "from" CIDR ranges
        let matches_from = rule.from.iter().any(|cidr_str| {
            Self::ip_matches_cidr(source_ip, cidr_str)
        });

        if !matches_from {
            return false;
        }

        // Check if target host matches any of the host patterns (regex)
        let matches_host = rule.to.hosts.iter().any(|pattern| {
            Self::host_matches_pattern(target_host, pattern)
        });

        if !matches_host {
            return false;
        }

        // Check if target port matches any of the port entries
        let matches_port = rule.to.ports.iter().any(|port_entry| {
            match port_entry {
                PortEntry::SinglePort(p) => *p == target_port,
                PortEntry::PortRange(start, end) => target_port >= *start && target_port <= *end,
            }
        });

        matches_port
    }

    fn ip_matches_cidr(ip: &IpAddr, cidr_str: &str) -> bool {
        // If CIDR string doesn't contain "/", treat it as a single host IP
        if !cidr_str.contains('/') {
            // Try to parse as IP address
            match IpAddr::from_str(cidr_str) {
                Ok(addr) => {
                    // If it's the same IP, it matches
                    if addr == *ip {
                        return true;
                    }
                    // Otherwise, create a network with full prefix length
                    let net = match addr {
                        IpAddr::V4(ipv4) => {
                            // IPv4: use /32 (single host)
                            match Ipv4Net::new(ipv4, 32) {
                                Ok(n) => IpNet::V4(n),
                                Err(_) => return false, // Should never happen with /32
                            }
                        }
                        IpAddr::V6(ipv6) => {
                            // IPv6: use /128 (single host)
                            match Ipv6Net::new(ipv6, 128) {
                                Ok(n) => IpNet::V6(n),
                                Err(_) => return false, // Should never happen with /128
                            }
                        }
                    };
                    return net.contains(ip);
                }
                Err(_) => return false, // Invalid IP format
            }
        }
        
        // If it contains "/", parse as CIDR notation
        match IpNet::from_str(cidr_str) {
            Ok(net) => net.contains(ip),
            Err(_) => false,
        }
    }

    fn host_matches_pattern(host: &str, pattern: &str) -> bool {
        debug!("matching host: {} pattern: {}", host, pattern);
        
        // Build regex with case-insensitive matching by default
        // Users can override this for specific parts using inline flags:
        // - (?-i) makes the following part case-sensitive
        // - (?i) is redundant but harmless (already case-insensitive)
        match RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
        {
            Ok(re) => re.is_match(host),
            Err(_) => false, // Invalid regex, doesn't match
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACLRule {
    #[serde(skip)]
    pub index: usize,
    #[serde(default = "default_priority")]
    pub priority: i32,
    pub action: ACLAction,
    pub from: Vec<String>,
    pub to: ACLTarget,
}

fn default_priority() -> i32 {
    0
}

#[derive(Debug, Clone)]
pub enum ACLAction {
    Allow,
    Deny,
}

impl<'de> Deserialize<'de> for ACLAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ACLActionVisitor;

        impl<'de> Visitor<'de> for ACLActionVisitor {
            type Value = ACLAction;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string: \"allow\" or \"deny\"")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value.to_lowercase().as_str() {
                    "allow" => Ok(ACLAction::Allow),
                    "deny" => Ok(ACLAction::Deny),
                    _ => Err(E::custom(format!("invalid action: expected \"allow\" or \"deny\", got: {}", value))),
                }
            }
        }

        deserializer.deserialize_str(ACLActionVisitor)
    }
}

impl Serialize for ACLAction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ACLAction::Allow => serializer.serialize_str("allow"),
            ACLAction::Deny => serializer.serialize_str("deny"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACLTarget {
    pub hosts: Vec<String>,
    pub ports: Vec<PortEntry>, // Can be numbers or strings like "8000-8100"
}

#[derive(Debug, Clone)]
pub enum PortEntry {
    SinglePort(u16),
    PortRange(u16, u16),
}

impl Serialize for PortEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            PortEntry::SinglePort(port) => serializer.serialize_u16(*port),
            PortEntry::PortRange(start, end) => {
                serializer.serialize_str(&format!("{}-{}", start, end))
            }
        }
    }
}

impl<'de> Deserialize<'de> for PortEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PortEntryVisitor;

        impl<'de> Visitor<'de> for PortEntryVisitor {
            type Value = PortEntry;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a port number (u16) or a port range string (e.g., \"8000-8100\")")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value > u16::MAX as u64 {
                    return Err(E::custom(format!("port number {} exceeds u16::MAX", value)));
                }
                Ok(PortEntry::SinglePort(value as u16))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value < 0 || value > u16::MAX as i64 {
                    return Err(E::custom(format!("port number {} is out of range for u16", value)));
                }
                Ok(PortEntry::SinglePort(value as u16))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                // Try to parse as a single port number first
                if let Ok(port) = value.parse::<u16>() {
                    return Ok(PortEntry::SinglePort(port));
                }

                // Try to parse as a range "start-end"
                if let Some(dash_pos) = value.find('-') {
                    let start_str = &value[..dash_pos].trim();
                    let end_str = &value[dash_pos + 1..].trim();
                    
                    let start = start_str.parse::<u16>()
                        .map_err(|_| E::custom(format!("invalid start port in range: {} (must be 0-65535)", start_str)))?;
                    let end = end_str.parse::<u16>()
                        .map_err(|_| E::custom(format!("invalid end port in range: {} (must be 0-65535)", end_str)))?;
                    
                    // Boundary checks: ports must be in range [0, 65535] (u16::MAX)
                    // Note: u16::parse already enforces this, but we add explicit checks for clarity
                    if start > u16::MAX as u16 {
                        return Err(E::custom(format!("start port {} exceeds maximum value 65535", start)));
                    }
                    if end > u16::MAX as u16 {
                        return Err(E::custom(format!("end port {} exceeds maximum value 65535", end)));
                    }
                    
                    // Range validation: end must be >= start
                    if end < start {
                        return Err(E::custom(format!("end port {} is less than start port {} in range", end, start)));
                    }
                    
                    return Ok(PortEntry::PortRange(start, end));
                }

                Err(E::custom(format!("invalid port entry: expected a number or range (e.g., \"8000-8100\"), got: {}", value)))
            }
        }

        deserializer.deserialize_any(PortEntryVisitor)
    }
}

pub fn load_acl<P: AsRef<Path>>(path: P) -> Result<ACLFile, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let mut acl_file: ACLFile = from_str(&content)?;
    
    // Set index for each rule based on their position in the file
    for (index, rule) in acl_file.acl.iter_mut().enumerate() {
        rule.index = index;
    }
    
    // Sort rules by priority (higher to lower), then by index (to maintain original order for same priority)
    acl_file.acl.sort_by(|a, b| {
        match b.priority.cmp(&a.priority) {
            std::cmp::Ordering::Equal => a.index.cmp(&b.index),
            other => other,
        }
    });
    
    Ok(acl_file)
}

pub fn serialize_acl_to_hcl(acl_file: &ACLFile) -> String {
    let mut output = String::new();
    
    for rule in &acl_file.acl {
        output.push_str("acl {\n");
        
        // Priority (only if not default)
        if rule.priority != 0 {
            output.push_str(&format!("  priority = {}\n", rule.priority));
        }
        
        // Action
        let action_str = match rule.action {
            ACLAction::Allow => "allow",
            ACLAction::Deny => "deny",
        };
        output.push_str(&format!("  action = \"{}\"\n", action_str));
        
        // From
        output.push_str("  from = [\n");
        for from_item in &rule.from {
            output.push_str(&format!("    \"{}\",\n", from_item));
        }
        output.push_str("  ]\n");
        
        // To
        output.push_str("  to {\n");
        
        // Hosts
        output.push_str("    hosts = [\n");
        for host in &rule.to.hosts {
            output.push_str(&format!("      \"{}\",\n", host));
        }
        output.push_str("    ]\n");
        
        // Ports
        output.push_str("    ports = [\n");
        for port in &rule.to.ports {
            match port {
                PortEntry::SinglePort(p) => {
                    output.push_str(&format!("      {},\n", p));
                }
                PortEntry::PortRange(start, end) => {
                    output.push_str(&format!("      \"{}-{}\"\n", start, end));
                }
            }
        }
        output.push_str("    ]\n");
        
        output.push_str("  }\n");
        output.push_str("}\n\n");
    }
    
    output
}


pub async fn is_acl_allowed(source_ip: &str, target: &str) -> Result<bool> {
    let acl = util::ACL.read().await;
    debug!("evaluating ACL, source ip: {} target: {}", source_ip, target);
    if !acl.is_some() {
        // no ACL set, allow by default
        debug!("no ACL set, allowing connection");
        return Ok(true);
    }
    let acl = acl.as_ref().unwrap();
    let result = acl.evaluate(source_ip, target);
    info!("evaluated ACL for source ip: {} target: {}, result: {:?}", source_ip, target, result);
    match result {
        ACLAction::Allow => Ok(true),
        ACLAction::Deny => Ok(false),
    }
}