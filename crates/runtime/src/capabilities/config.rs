//! Trusted operator grants. Agent requests can only reduce this authority.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capabilities {
    pub network_origins: BTreeSet<String>,
    pub tools: BTreeSet<String>,
    pub memory_scopes: BTreeSet<String>,
    pub child_agents: BTreeSet<String>,
}
impl Capabilities {
    pub fn intersection(&self, grants: &Self) -> Self {
        fn intersect(a: &BTreeSet<String>, b: &BTreeSet<String>) -> BTreeSet<String> {
            a.intersection(b).cloned().collect()
        }
        Self {
            network_origins: intersect(&self.network_origins, &grants.network_origins),
            tools: intersect(&self.tools, &grants.tools),
            memory_scopes: intersect(&self.memory_scopes, &grants.memory_scopes),
            child_agents: intersect(&self.child_agents, &grants.child_agents),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        for origin in &self.network_origins {
            let url = url::Url::parse(origin).map_err(|_| "invalid network origin")?;
            if origin.contains('*')
                || !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.origin().ascii_serialization() != *origin
            {
                return Err(format!(
                    "network grant must be an exact canonical HTTP origin: {origin}"
                ));
            }
        }
        for name in self
            .tools
            .iter()
            .chain(&self.memory_scopes)
            .chain(&self.child_agents)
        {
            if name.is_empty()
                || name.len() > 256
                || name.contains('*')
                || name.chars().any(char::is_control)
            {
                return Err(
                    "capability names must be nonempty exact names without wildcards".into(),
                );
            }
        }
        Ok(())
    }
    pub fn require_network(&self, target: &str) -> Result<(), String> {
        let url = url::Url::parse(target).map_err(|_| "permission denied: invalid network URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || !self
                .network_origins
                .contains(&url.origin().ascii_serialization())
        {
            return Err("permission denied: network origin".into());
        }
        Ok(())
    }
    pub fn require_tool(&self, name: &str) -> Result<(), String> {
        require(&self.tools, name, "tool")
    }
    pub fn require_memory_scope(&self, name: &str) -> Result<(), String> {
        require(&self.memory_scopes, name, "memory scope")
    }
    pub fn require_child_agent(&self, name: &str) -> Result<(), String> {
        require(&self.child_agents, name, "child agent")
    }
}
fn require(grants: &BTreeSet<String>, name: &str, kind: &str) -> Result<(), String> {
    if grants.contains(name) {
        Ok(())
    } else {
        Err(format!("permission denied: {kind} {name}"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    pub grants: Capabilities,
    /// Credential names map to environment-variable names, never secret values.
    pub credentials: BTreeMap<String, String>,
}
impl HostConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.grants.validate()?;
        for reference in self.credentials.values() {
            let mut bytes = reference.bytes();
            if !bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err("credentials must reference environment-variable names".into());
            }
        }
        Ok(())
    }
    pub fn from_yaml(text: &str) -> Result<Self, String> {
        let mut value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
        if let Some(map) = value.as_mapping_mut() {
            if let Some(permissions) = map.remove(serde_yaml::Value::String("permissions".into())) {
                let permissions: Vec<String> =
                    serde_yaml::from_value(permissions).map_err(|e| e.to_string())?;
                if !permissions.is_empty() {
                    return Err("scoped grants cannot be combined with legacy permissions".into());
                }
            }
        }
        let config: Self = serde_yaml::from_value(value).map_err(|e| e.to_string())?;
        config.validate()?;
        Ok(config)
    }
    /// Missing/scoped-free files retain the legacy Policy loading behavior.
    pub fn load_optional(path: &Path) -> Result<Option<Self>, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let value: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
        let mapping = value
            .as_mapping()
            .ok_or("configuration must be a mapping")?;
        if mapping
            .keys()
            .all(|key| key.as_str() == Some("permissions"))
        {
            return Ok(None);
        }
        Self::from_yaml(&text).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn misspelled_scoped_keys_never_fall_back_to_broad_legacy_policy() {
        let path =
            std::env::temp_dir().join(format!("ondure-config-{}.yaml", uuid::Uuid::new_v4()));
        for text in [
            "grant: {}",
            "permissions: [net]\ngrant: {}",
            "credential: API_KEY",
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(HostConfig::load_optional(&path).is_err());
        }
        std::fs::write(&path, "permissions: [net]").unwrap();
        assert!(HostConfig::load_optional(&path).unwrap().is_none());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn requests_can_only_attenuate_grants() {
        let grants = HostConfig::from_yaml("grants:\n  tools: [read]\n  network_origins: ['https://example.com']\n  memory_scopes: [team]\n  child_agents: [worker]").unwrap().grants;
        let requested = Capabilities {
            tools: ["read".into(), "pay".into()].into(),
            ..Default::default()
        };
        let effective = requested.intersection(&grants);
        assert!(effective.require_tool("read").is_ok());
        assert!(effective.require_tool("pay").is_err());
        assert!(effective.require_network("https://example.com").is_err());
        assert!(effective.require_memory_scope("team").is_err());
        assert!(effective.require_child_agent("worker").is_err());
    }
    #[test]
    fn exact_origins_do_not_grant_subdomains_ports_or_userinfo() {
        let grants = HostConfig::from_yaml("grants:\n  network_origins: ['https://example.com']")
            .unwrap()
            .grants;
        assert!(grants
            .require_network("https://example.com/path?q=1")
            .is_ok());
        for url in [
            "https://evil.example.com",
            "https://example.com:444",
            "http://example.com",
            "https://user@example.com",
        ] {
            assert!(grants.require_network(url).is_err());
        }
        for origin in [
            "https://example.com/path",
            "https://example.com/",
            "https://example.com:443",
            "https://*.example.com",
        ] {
            assert!(
                HostConfig::from_yaml(&format!("grants:\n  network_origins: ['{origin}']"))
                    .is_err()
            );
        }
    }
    #[test]
    fn secret_values_and_unknown_fields_are_rejected() {
        assert!(HostConfig::from_yaml("credentials: { api: sk-secret }").is_err());
        assert!(HostConfig::from_yaml("credentials: { api: API_KEY }").is_ok());
        assert!(HostConfig::from_yaml("grants: {tool: [read]}").is_err());
        assert!(HostConfig::from_yaml("permissions: [net]\ngrants: {}").is_err());
    }
}
