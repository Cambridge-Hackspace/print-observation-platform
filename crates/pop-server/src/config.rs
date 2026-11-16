//! Configuration, from the environment.
//!
//! | Variable       | Default          | What it is                                  |
//! | -------------- | ---------------- | ------------------------------------------- |
//! | `POP_LISTEN`   | `127.0.0.1:8080` | The address the server listens on.          |
//! | `POP_DATA_DIR` | `./data`         | The database and uploaded prints live here. |

use std::{fmt, net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
}

/// A setting that could not be used, named so the message says which.
#[derive(Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub variable: &'static str,
    pub value: String,
    pub reason: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={:?}: {}", self.variable, self.value, self.reason)
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Read the configuration from the process's environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Read the configuration through `get`, which returns a variable's value
    /// or `None` when it is unset.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let listen = match get("POP_LISTEN") {
            None => SocketAddr::from(([127, 0, 0, 1], 8080)),
            Some(v) => v
                .parse()
                .map_err(|e: std::net::AddrParseError| ConfigError {
                    variable: "POP_LISTEN",
                    value: v.clone(),
                    reason: format!("{e}; expected an address and port, such as 127.0.0.1:8080"),
                })?,
        };
        let data_dir = match get("POP_DATA_DIR") {
            None => PathBuf::from("data"),
            Some(v) if v.is_empty() => {
                return Err(ConfigError {
                    variable: "POP_DATA_DIR",
                    value: v,
                    reason: "is empty; unset it for ./data, or name a directory".into(),
                });
            }
            Some(v) => PathBuf::from(v),
        };
        Ok(Self { listen, data_dir })
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("pop.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        Config::from_lookup(|k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
    }

    #[test]
    fn unset_means_loopback_and_data() {
        let c = with(&[]).unwrap();
        assert_eq!(c.listen, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(c.database_path(), PathBuf::from("data/pop.db"));
    }

    #[test]
    fn set_values_are_used() {
        let c = with(&[
            ("POP_LISTEN", "0.0.0.0:9000"),
            ("POP_DATA_DIR", "/var/lib/pop"),
        ])
        .unwrap();
        assert_eq!(c.listen, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(c.database_path(), PathBuf::from("/var/lib/pop/pop.db"));
    }

    #[test]
    fn a_listen_address_without_a_port_names_the_variable() {
        let e = with(&[("POP_LISTEN", "0.0.0.0")]).unwrap_err();
        assert_eq!(e.variable, "POP_LISTEN");
        assert!(e.to_string().starts_with("POP_LISTEN=\"0.0.0.0\": "), "{e}");
    }

    #[test]
    fn an_empty_data_dir_is_refused_rather_than_read_as_the_cwd() {
        let e = with(&[("POP_DATA_DIR", "")]).unwrap_err();
        assert_eq!(e.variable, "POP_DATA_DIR");
    }
}
