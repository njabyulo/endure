//! Policy parsing and permission decisions, independent of V8 execution.
//! Tests below demonstrate default denial and rejection of unknown permissions.

use serde::Deserialize;
use std::{fmt, fs, path::Path};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub net: bool,
}

#[derive(Debug)]
pub enum PolicyError {
    UnknownPermission(String),
    Yaml(serde_yaml::Error),
    Io(std::io::Error),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPermission(name) => write!(f, "unknown permission: {name}"),
            Self::Yaml(error) => write!(f, "invalid policy: {error}"),
            Self::Io(error) => write!(f, "cannot read policy: {error}"),
        }
    }
}

impl std::error::Error for PolicyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Yaml(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::UnknownPermission(_) => None,
        }
    }
}

#[derive(Deserialize)]
struct PolicyFile {
    #[serde(default)]
    permissions: Vec<String>,
}

impl Policy {
    pub fn from_yaml(text: &str) -> Result<Self, PolicyError> {
        let file: PolicyFile = serde_yaml::from_str(text).map_err(PolicyError::Yaml)?;
        let mut policy = Self::default();
        for name in file.permissions {
            match name.as_str() {
                "net" => policy.net = true,
                _ => return Err(PolicyError::UnknownPermission(name)),
            }
        }
        Ok(policy)
    }

    pub fn load(path: &Path) -> Result<Self, PolicyError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_yaml(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(PolicyError::Io(error)),
        }
    }

    pub fn allows_net(&self) -> bool {
        self.net
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_permissions_deny_net() {
        assert!(!Policy::from_yaml("permissions: []").unwrap().allows_net());
    }

    #[test]
    fn net_permission_allows_net() {
        assert!(Policy::from_yaml("permissions: [net]")
            .unwrap()
            .allows_net());
    }

    #[test]
    fn unknown_permission_fails_to_load() {
        assert!(matches!(
            Policy::from_yaml("permissions: [fs]"),
            Err(PolicyError::UnknownPermission(name)) if name == "fs"
        ));
    }

    #[test]
    fn missing_file_denies_net() {
        let path = std::env::temp_dir().join(format!(
            "ondure-missing-{}-{}.yaml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(Policy::load(&path).unwrap(), Policy { net: false });
    }

    #[test]
    fn omitted_permissions_deny_net() {
        assert_eq!(Policy::from_yaml("{}").unwrap(), Policy::default());
    }

    #[test]
    fn malformed_policy_fails_to_load() {
        for text in ["permissions: [", "permissions: net", "permissions: [123]"] {
            assert!(Policy::from_yaml(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn read_errors_are_not_missing_files() {
        assert!(matches!(
            Policy::load(&std::env::temp_dir()),
            Err(PolicyError::Io(_))
        ));
    }
}
