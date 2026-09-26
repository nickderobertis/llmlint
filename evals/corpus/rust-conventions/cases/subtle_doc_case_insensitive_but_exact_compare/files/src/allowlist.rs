use std::collections::HashSet;

/// Email domains allowed to sign up.
#[derive(Debug, Default, Clone)]
pub struct DomainAllowlist {
    domains: HashSet<String>,
}

impl DomainAllowlist {
    /// An allowlist of `domains`, stored lowercased.
    pub fn new(domains: &[&str]) -> DomainAllowlist {
        DomainAllowlist { domains: domains.iter().map(|domain| domain.to_lowercase()).collect() }
    }

    /// Whether `email`'s domain is allowed. The domain comparison is
    /// case-insensitive, so `Ann@Example.COM` matches `example.com`.
    pub fn allows(&self, email: &str) -> bool {
        match email.rsplit_once('@') {
            Some((_, domain)) => self.domains.contains(domain),
            None => false,
        }
    }
}
