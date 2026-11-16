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
    pub bam: Bam,
    pub secrets: Secrets,
    pub guides: Guides,
}

/// Where POP reads its printers (#20, #21). Off while `url` is empty.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Bam {
    /// BAM!'s address: https://bam.example.org. Empty: no BAM!.
    pub url: String,
    /// POP's API token. A secret: give it as POP_BAM_TOKEN rather than here.
    pub token: String,
    /// The id of the BAM! category whose assets are the printers.
    pub category: String,
    /// Which of the category's fields give each printer's settings.
    pub fields: pop_integrations::printers::FieldMap,
}

/// How published guides are offered to everyone (#94).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Guides {
    /// The license each published guide is shown under: "CC BY-SA 4.0".
    /// Empty: none is shown.
    pub license: String,
    /// Whether search engines may index the public guides. Off: every
    /// public guide answer says noindex.
    pub indexable: bool,
}

/// Where secrets POP keeps (a Bambu printer's access code) are sealed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Secrets {
    /// 32 random bytes in base64 (`openssl rand -base64 32`). A secret: give
    /// it as POP_SECRETS_KEY. Empty: POP refuses to keep secrets.
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Server {
    /// The address and port the server listens on.
    pub listen: SocketAddr,
    /// The address phones reach POP at: https://pop.example.org. The tower
    /// shows it as a QR code (#49). Empty: no QR code.
    pub public_url: String,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            public_url: String::new(),
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
    /// Reverse proxies whose forwarding headers are believed. A request that
    /// carries `Forwarded` or `X-Forwarded-For` from any other peer is remote,
    /// whatever its address: a proxy on this machine would otherwise make every
    /// request it passes on look local.
    pub trusted_proxies: Vec<IpNet>,
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
        config.validate_whole()?;
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
            self.access.local_networks = networks("POP_ACCESS_LOCAL_NETWORKS", &v)?;
        }
        if let Some(v) = env("POP_ACCESS_TRUSTED_PROXIES") {
            self.access.trusted_proxies = networks("POP_ACCESS_TRUSTED_PROXIES", &v)?;
        }
        let set = |var: &str, slot: &mut String| {
            if let Some(v) = env(var) {
                *slot = v;
            }
        };
        set("POP_SERVER_PUBLIC_URL", &mut self.server.public_url);
        set("POP_BAM_URL", &mut self.bam.url);
        set("POP_BAM_TOKEN", &mut self.bam.token);
        set("POP_BAM_CATEGORY", &mut self.bam.category);
        let f = &mut self.bam.fields;
        set("POP_BAM_FIELDS_CONNECTION", &mut f.connection);
        set("POP_BAM_FIELDS_ADDRESS", &mut f.address);
        set("POP_BAM_FIELDS_CAMERA", &mut f.camera);
        set("POP_BAM_FIELDS_MODEL", &mut f.model);
        set("POP_BAM_FIELDS_MATERIALS", &mut f.materials);
        set("POP_BAM_FIELDS_TRAINING", &mut f.training);
        set("POP_BAM_FIELDS_SERIAL", &mut f.serial);
        set("POP_SECRETS_KEY", &mut self.secrets.key);
        set("POP_GUIDES_LICENSE", &mut self.guides.license);
        if let Some(v) = env("POP_GUIDES_INDEXABLE") {
            self.guides.indexable = match v.as_str() {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(err(
                        "POP_GUIDES_INDEXABLE",
                        format!("{v:?}: expected true or false"),
                    ));
                }
            };
        }
        // Each override that has a form is checked here, so its error names
        // the variable rather than the file's key.
        if env("POP_SERVER_PUBLIC_URL").is_some()
            && !self.server.public_url.is_empty()
            && !is_http(&self.server.public_url)
        {
            return Err(err(
                "POP_SERVER_PUBLIC_URL",
                format!(
                    "{:?}: expected the http:// or https:// address phones reach POP at",
                    self.server.public_url
                ),
            ));
        }
        if env("POP_BAM_URL").is_some() && !self.bam.url.is_empty() && !is_http(&self.bam.url) {
            return Err(err(
                "POP_BAM_URL",
                format!(
                    "{:?}: expected an http:// or https:// address",
                    self.bam.url
                ),
            ));
        }
        if env("POP_SECRETS_KEY").is_some()
            && !self.secrets.key.is_empty()
            && !is_key(&self.secrets.key)
        {
            return Err(err(
                "POP_SECRETS_KEY",
                "must be 32 bytes in base64, as `openssl rand -base64 32` makes",
            ));
        }
        Ok(())
    }

    /// Checks across keys, once the environment's overrides are in: a file
    /// may leave a secret to the environment.
    fn validate_whole(&self) -> Result<(), ConfigError> {
        let u = &self.server.public_url;
        if !u.is_empty() && !is_http(u) {
            return Err(err(
                "server.public_url",
                format!("{u:?}: expected the http:// or https:// address phones reach POP at"),
            ));
        }
        let b = &self.bam;
        if !b.url.is_empty() {
            if !is_http(&b.url) {
                return Err(err(
                    "bam.url",
                    format!("{:?}: expected an http:// or https:// address", b.url),
                ));
            }
            if b.token.is_empty() {
                return Err(err(
                    "bam.token",
                    "is empty, with bam.url set: give POP's BAM! token as POP_BAM_TOKEN",
                ));
            }
            if b.category.is_empty() {
                return Err(err(
                    "bam.category",
                    "is empty, with bam.url set: name the printer category's id",
                ));
            }
        }
        let f = &b.fields;
        for (key, v) in [
            ("bam.fields.connection", &f.connection),
            ("bam.fields.address", &f.address),
            ("bam.fields.camera", &f.camera),
            ("bam.fields.model", &f.model),
            ("bam.fields.materials", &f.materials),
            ("bam.fields.training", &f.training),
            ("bam.fields.serial", &f.serial),
        ] {
            if v.is_empty() {
                return Err(err(
                    key,
                    "is empty: name the BAM! field's key, or leave it out for the default",
                ));
            }
        }
        if !self.secrets.key.is_empty() && !is_key(&self.secrets.key) {
            return Err(err(
                "secrets.key",
                "must be 32 bytes in base64, as `openssl rand -base64 32` makes",
            ));
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

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// 32 bytes in base64.
fn is_key(k: &str) -> bool {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(k)
        .is_ok_and(|k| k.len() == 32)
}

/// A comma-separated list of networks from an environment variable.
fn networks(var: &str, v: &str) -> Result<Vec<IpNet>, ConfigError> {
    v.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse().map_err(|_| {
                err(
                    var,
                    format!("{s:?} is not a network: expected CIDR notation, such as 192.168.1.0/24, comma-separated"),
                )
            })
        })
        .collect()
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
            ("POP_ACCESS_TRUSTED_PROXIES", "a-proxy"),
            ("POP_SECRETS_KEY", "c2hvcnQ="),
            ("POP_BAM_URL", "bam.example.org"),
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

    #[test]
    fn bam_needs_its_token_and_category_once_it_has_an_address() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let e = Config::load(None, env(&[("POP_BAM_URL", "https://bam.example.org")])).unwrap_err();
        assert_eq!(e.key, "bam.token");
        let e = Config::load(
            None,
            env(&[
                ("POP_BAM_URL", "https://bam.example.org"),
                ("POP_BAM_TOKEN", "bam_x"),
            ]),
        )
        .unwrap_err();
        assert_eq!(e.key, "bam.category");
        let e = Config::load(None, env(&[("POP_BAM_URL", "bam.example.org")])).unwrap_err();
        assert_eq!(
            e.key, "POP_BAM_URL",
            "an override's error names its variable"
        );
        let e = Config::parse("[bam]\nurl = \"bam.example.org\"\n")
            .and_then(|c| c.validate_whole().map(|()| c))
            .unwrap_err();
        assert_eq!(e.key, "bam.url", "and the file's names its key");
        let c = Config::load(
            None,
            env(&[
                ("POP_BAM_URL", "https://bam.example.org"),
                ("POP_BAM_TOKEN", "bam_x"),
                ("POP_BAM_CATEGORY", "c1"),
            ]),
        )
        .unwrap();
        assert_eq!(c.bam.token, "bam_x");
    }

    #[test]
    fn the_public_address_must_be_one_and_its_error_names_where_it_came_from() {
        let e = env_err(&[("POP_SERVER_PUBLIC_URL", "pop.example.org")]);
        assert_eq!(e.key, "POP_SERVER_PUBLIC_URL");
        let e = Config::parse("[server]\npublic_url = \"pop.example.org\"\n")
            .and_then(|c| c.validate_whole().map(|()| c))
            .unwrap_err();
        assert_eq!(e.key, "server.public_url");
        let c = Config::parse("[server]\npublic_url = \"https://pop.example.org\"\n").unwrap();
        assert_eq!(c.server.public_url, "https://pop.example.org");
        let c = Config::load(None, |k| {
            (k == "POP_SERVER_PUBLIC_URL").then(|| "http://10.0.0.5:8080".to_owned())
        })
        .unwrap();
        assert_eq!(c.server.public_url, "http://10.0.0.5:8080");
    }

    #[test]
    fn guides_settings_come_from_the_file_or_the_environment() {
        let c = Config::parse("[guides]\nlicense = \"CC BY-SA 4.0\"\nindexable = true\n").unwrap();
        assert_eq!(c.guides.license, "CC BY-SA 4.0");
        assert!(c.guides.indexable);
        assert!(
            !Config::default().guides.indexable,
            "not indexed unless said"
        );
        let e = env_err(&[("POP_GUIDES_INDEXABLE", "yes")]);
        assert_eq!(e.key, "POP_GUIDES_INDEXABLE");
        let c = Config::load(None, |k| match k {
            "POP_GUIDES_INDEXABLE" => Some("true".to_owned()),
            "POP_GUIDES_LICENSE" => Some("CC BY 4.0".to_owned()),
            _ => None,
        })
        .unwrap();
        assert!(c.guides.indexable);
        assert_eq!(c.guides.license, "CC BY 4.0");
    }

    #[test]
    fn a_file_may_leave_bams_token_to_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pop.toml");
        std::fs::write(
            &path,
            "[bam]\nurl = \"https://bam.example.org\"\ncategory = \"c1\"\n",
        )
        .unwrap();
        let c = Config::load(Some(&path), |k| {
            (k == "POP_BAM_TOKEN").then(|| "bam_x".to_owned())
        })
        .unwrap();
        assert_eq!(c.bam.token, "bam_x");
    }

    #[test]
    fn a_renamed_bam_field_is_used_and_an_empty_one_named() {
        let c = Config::parse("[bam.fields]\nconnection = \"link\"\n").unwrap();
        assert_eq!(c.bam.fields.connection, "link");
        assert_eq!(
            c.bam.fields.address, "address",
            "the rest keep their defaults"
        );
        let e =
            Config::load(None, |k| (k == "POP_BAM_FIELDS_ADDRESS").then(String::new)).unwrap_err();
        assert_eq!(e.key, "bam.fields.address");
    }

    #[test]
    fn a_secrets_key_must_be_32_bytes() {
        let key = |v: &'static str| move |k: &str| (k == "POP_SECRETS_KEY").then(|| v.to_owned());
        assert_eq!(
            Config::load(None, key("dG9vIHNob3J0")).unwrap_err().key,
            "POP_SECRETS_KEY"
        );
        let e = Config::parse("[secrets]\nkey = \"dG9vIHNob3J0\"\n")
            .and_then(|c| c.validate_whole().map(|()| c))
            .unwrap_err();
        assert_eq!(e.key, "secrets.key");
        assert!(Config::load(None, key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")).is_ok());
    }
}
