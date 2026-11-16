//! Configuration: one TOML file, validated at startup (#2), with environment
//! overrides for secrets and container deployments.
//!
//! Every value has a default, so a missing file is a working configuration
//! for development; `pop.example.toml` documents each key with its default.
//! An override is `POP_<SECTION>_<KEY>`, upper-cased: `POP_SERVER_LISTEN`
//! overrides `server.listen`.
//!
//! Every error names what is wrong: the dotted key for a value in the file,
//! or the variable for an override, and what was expected.

use std::{
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use ipnet::IpNet;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub server: Server,
    pub storage: Storage,
    pub log: Log,
    pub access: Access,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Server {
    /// The address and port the server listens on.
    pub listen: SocketAddr,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Storage {
    /// Where the database and the stored print files live.
    pub data_dir: PathBuf,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Lines for a person reading a terminal.
    #[default]
    Text,
    /// One JSON object per line, for a log collector.
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Log {
    pub level: LogLevel,
    pub format: LogFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Access {
    /// The space's own networks. A request from one of these (or from a kiosk
    /// session) is local, and may control printers (ADR 0001, "Control is
    /// local"). Empty means only loopback is local.
    pub local_networks: Vec<IpNet>,
}

/// What is wrong, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// The dotted key in the file (`server.listen`), the environment variable
    /// (`POP_SERVER_LISTEN`), or the file itself when it cannot be read.
    pub key: String,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.message)
    }
}

impl std::error::Error for ConfigError {}

fn err(key: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError {
        key: key.into(),
        message: message.into(),
    }
}

/// The file read when none is named, if it exists.
pub const DEFAULT_PATH: &str = "pop.toml";

impl Config {
    /// Parse and validate a configuration file's text. `origin` names it in
    /// errors about the file as a whole.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let de = toml::Deserializer::new(text);
        let config: Config = serde_path_to_error::deserialize(de).map_err(|e| {
            let key = e.path().to_string();
            let inner = e.into_inner();
            // toml's message carries the line and a caret; the first line
            // after its header is the part worth showing.
            let message = inner.message().trim().to_owned();
            if key == "." || key.is_empty() {
                err("the configuration", message)
            } else {
                err(key, message)
            }
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Read the configuration: the file at `path` if one is named (it must
    /// exist), else `pop.toml` if it exists, else the defaults; then the
    /// environment's overrides, read through `env`.
    pub fn load(
        path: Option<&Path>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let mut config = match path {
            Some(p) => Self::parse(&read(p)?)?,
            None if Path::new(DEFAULT_PATH).exists() => {
                Self::parse(&read(Path::new(DEFAULT_PATH))?)?
            }
            None => Self::default(),
        };
        config.apply_env(env)?;
        config.validate()?;
        Ok(config)
    }

    fn apply_env(&mut self, env: impl Fn(&str) -> Option<String>) -> Result<(), ConfigError> {
        if let Some(v) = env("POP_SERVER_LISTEN") {
            self.server.listen = v.parse().map_err(|e| {
                err(
                    "POP_SERVER_LISTEN",
                    format!("{e}: expected an address and port, such as 127.0.0.1:8080, not {v:?}"),
                )
            })?;
        }
        if let Some(v) = env("POP_STORAGE_DATA_DIR") {
            self.storage.data_dir = PathBuf::from(v);
            if self.storage.data_dir.as_os_str().is_empty() {
                return Err(err(
                    "POP_STORAGE_DATA_DIR",
                    "is empty: unset it for the configured directory, or name one",
                ));
            }
        }
        if let Some(v) = env("POP_LOG_LEVEL") {
            self.log.level = enum_value("POP_LOG_LEVEL", &v)?;
        }
        if let Some(v) = env("POP_LOG_FORMAT") {
            self.log.format = enum_value("POP_LOG_FORMAT", &v)?;
        }
        if let Some(v) = env("POP_ACCESS_LOCAL_NETWORKS") {
            self.access.local_networks = v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse().map_err(|_| {
                        err(
                            "POP_ACCESS_LOCAL_NETWORKS",
                            format!("{s:?} is not a network: expected CIDR notation, such as 192.168.1.0/24, comma-separated"),
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
        }
        Ok(())
    }

    /// Checks no type can express.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.storage.data_dir.as_os_str().is_empty() {
            return Err(err(
                "storage.data_dir",
                "is empty: name a directory, or leave the key out for ./data",
            ));
        }
        Ok(())
    }

    pub fn database_path(&self) -> PathBuf {
        self.storage.data_dir.join("pop.db")
    }
}

fn read(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path)
        .map_err(|e| err(path.display().to_string(), format!("cannot be read: {e}")))
}

/// An enum's value from an environment variable, through the same names the
/// file uses, so the two cannot disagree.
fn enum_value<T: for<'de> Deserialize<'de>>(var: &str, v: &str) -> Result<T, ConfigError> {
    T::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(v))
        .map_err(|e| err(var, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_err(text: &str) -> ConfigError {
        Config::parse(text).expect_err(text)
    }

    fn env_err(vars: &[(&str, &str)]) -> ConfigError {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        Config::load(None, |k| {
            vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
        })
        .expect_err("an error")
    }

    // ---- every kind of invalid value names its key ----------------------------

    #[test]
    fn an_unknown_key_is_named() {
        let e = parse_err("[server]\nlisten = \"127.0.0.1:8080\"\nlsiten = \"x\"\n");
        assert_eq!(e.key, "server.lsiten");
        assert!(
            e.message.contains("listen"),
            "the message offers the right key: {e}"
        );
    }

    #[test]
    fn an_unknown_section_is_named() {
        let e = parse_err("[serverr]\nlisten = \"127.0.0.1:8080\"\n");
        assert_eq!(e.key, "serverr");
        assert!(
            e.message.contains("`server`"),
            "the message offers the right section: {e}"
        );
    }

    #[test]
    fn a_wrong_type_names_the_key_and_what_was_expected() {
        let e = parse_err("[server]\nlisten = 8080\n");
        assert_eq!(e.key, "server.listen");
        assert!(e.message.contains("expected"), "{e}");
    }

    #[test]
    fn a_listen_address_without_a_port_names_the_key() {
        let e = parse_err("[server]\nlisten = \"0.0.0.0\"\n");
        assert_eq!(e.key, "server.listen");
    }

    #[test]
    fn an_empty_data_dir_names_the_key() {
        let e = parse_err("[storage]\ndata_dir = \"\"\n");
        assert_eq!(e.key, "storage.data_dir");
    }

    #[test]
    fn an_unknown_log_level_names_the_key_and_the_choices() {
        let e = parse_err("[log]\nlevel = \"loud\"\n");
        assert_eq!(e.key, "log.level");
        assert!(e.message.contains("debug"), "{e}");
    }

    #[test]
    fn an_unknown_log_format_names_the_key() {
        let e = parse_err("[log]\nformat = \"xml\"\n");
        assert_eq!(e.key, "log.format");
    }

    #[test]
    fn a_network_that_is_not_cidr_names_the_key_and_its_place() {
        let e = parse_err("[access]\nlocal_networks = [\"10.0.0.0/8\", \"the-shop\"]\n");
        assert_eq!(e.key, "access.local_networks[1]");
    }

    #[test]
    fn text_that_is_not_toml_is_refused() {
        let e = parse_err("[server\n");
        assert_eq!(e.key, "the configuration");
    }

    #[test]
    fn a_named_file_that_is_missing_is_named() {
        let e = Config::load(Some(Path::new("/no/such/pop.toml")), |_| None).unwrap_err();
        assert_eq!(e.key, "/no/such/pop.toml");
    }

    #[test]
    fn a_bad_override_names_its_variable() {
        for (var, value) in [
            ("POP_SERVER_LISTEN", "0.0.0.0"),
            ("POP_STORAGE_DATA_DIR", ""),
            ("POP_LOG_LEVEL", "loud"),
            ("POP_LOG_FORMAT", "xml"),
            ("POP_ACCESS_LOCAL_NETWORKS", "10.0.0.0/8, the-shop"),
        ] {
            assert_eq!(env_err(&[(var, value)]).key, var, "{var}={value:?}");
        }
    }

    // ---- what valid configuration means -----------------------------------------

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn values_in_the_file_are_used() {
        let c = Config::parse(
            "[server]\nlisten = \"0.0.0.0:9000\"\n[storage]\ndata_dir = \"/var/lib/pop\"\n\
             [log]\nlevel = \"debug\"\nformat = \"json\"\n[access]\nlocal_networks = [\"192.168.1.0/24\", \"fd00::/8\"]\n",
        )
        .unwrap();
        assert_eq!(c.server.listen, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(c.database_path(), PathBuf::from("/var/lib/pop/pop.db"));
        assert_eq!(c.log.level, LogLevel::Debug);
        assert_eq!(c.log.format, LogFormat::Json);
        assert_eq!(c.access.local_networks.len(), 2);
    }

    #[test]
    fn an_override_beats_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pop.toml");
        std::fs::write(
            &path,
            "[server]\nlisten = \"127.0.0.1:1\"\n[log]\nlevel = \"warn\"\n",
        )
        .unwrap();
        let c = Config::load(Some(&path), |k| match k {
            "POP_SERVER_LISTEN" => Some("0.0.0.0:2".into()),
            "POP_ACCESS_LOCAL_NETWORKS" => Some("10.0.0.0/8,192.168.0.0/16".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.server.listen, "0.0.0.0:2".parse().unwrap());
        assert_eq!(
            c.log.level,
            LogLevel::Warn,
            "a key with no override keeps the file's value"
        );
        assert_eq!(c.access.local_networks.len(), 2);
    }

    #[test]
    fn the_sample_documents_exactly_the_defaults() {
        let sample = include_str!("../../../pop.example.toml");
        assert_eq!(Config::parse(sample).unwrap(), Config::default());
    }
}
