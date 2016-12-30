//! Support for log4rs configuration from files.
//!
//! Multiple file formats are supported, each requiring a Cargo feature to be
//! enabled. YAML support requires the `yaml` feature, JSON support requires
//! the `JSON` feature, and TOML support requires the `toml` feature.
//!
//! # Syntax
//!
//! All file formats currently share the same structure. The example below is
//! of the YAML format.
//!
//! ```yaml
//! # If set, log4rs will scan the file at the specified rate for changes and
//! # automatically reconfigure the logger. The input string is parsed by the
//! # humantime crate.
//! refresh_rate: 30 seconds
//!
//! # The "appenders" map contains the set of appenders, indexed by their names.
//! appenders:
//!
//!   foo:
//!
//!     # All appenders must specify a "kind", which will be used to look up the
//!     # logic to construct the appender in the `Deserializers` passed to the
//!     # deserialization function.
//!     kind: console
//!
//!     # Filters attached to an appender are specified inside the "filters"
//!     # array.
//!     filters:
//!
//!       -
//!         # Like appenders, filters are identified by their "kind".
//!         kind: threshold
//!
//!         # The remainder of the configuration is passed along to the
//!         # filter's builder, and will vary based on the kind of filter.
//!         level: error
//!
//!     # The remainder of the configuration is passed along to the appender's
//!     # builder, and will vary based on the kind of appender.
//!     # Appenders will commonly be associated with an encoder.
//!     encoder:
//!
//!       # Like appenders, encoders are identified by their "kind". If no kind
//!       # is specified, it will default to "pattern".
//!       kind: pattern
//!
//!       # The remainder of the configuration is passed along to the
//!       # encoder's builder, and will vary based on the kind of encoder.
//!       pattern: "{d} [{t}] {m}{n}"
//!
//! # The root logger is configured by the "root" map. Defaults to a level of
//! # "debug" and no appenders if not provided.
//! root:
//!
//!   # The maximum log level for the root logger.
//!   level: warn
//!
//!   # The list of appenders attached to the root logger. Defaults to an empty
//!   # list if not specified.
//!   appenders:
//!     - foo
//!
//! # The "loggers" map contains the set of configured loggers, indexed by their
//! # names.
//! loggers:
//!
//!   foo::bar::baz:
//!
//!     # The maximum log level. Defaults to the level of the logger's parent if
//!     # not specified.
//!     level: trace
//!
//!     # The list of appenders attached to the logger. Defaults to an empty
//!     # list if not specified.
//!     appenders:
//!       - foo
//!
//!     # The additivity of the logger. If true, appenders attached to the
//!     # logger's parent will also be attached to this logger. Defaults to true
//!     # if not specified.
//!     additive: false
//! ```

use log::LogLevelFilter;
use std::any::Any;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::error;
use std::fmt;
use std::time::Duration;
use std::sync::Arc;
use typemap::{Key, ShareCloneMap};
use serde;
use serde_value::Value;

use PrivateConfigErrorsExt;
use config;
use filter::FilterConfig;

mod raw;

/// A trait implemented by traits which are deserializable.
pub trait Deserializable: Any {
    /// Returns a name for objects implementing the trait suitable for display in error messages.
    ///
    /// For example, the `Deserializable` implementation for the `Append` trait returns "appender".
    fn name() -> &'static str;
}

/// A trait for objects that can deserialize log4rs components out of a config.
pub trait Deserialize: Send + Sync + 'static {
    /// The trait that this deserializer will create.
    type Trait: ?Sized + Deserializable;

    /// This deserializer's configuration.
    type Config: serde::Deserialize;

    /// Create a new trait object based on the provided config.
    fn deserialize(&self,
                   config: Self::Config,
                   deserializers: &Deserializers)
                   -> Result<Box<Self::Trait>, Box<error::Error + Sync + Send>>;
}

trait ErasedDeserialize: Send + Sync + 'static {
    type Trait: ?Sized;

    fn deserialize(&self,
                   config: Value,
                   deserializers: &Deserializers)
                   -> Result<Box<Self::Trait>, Box<error::Error + Sync + Send>>;
}

struct DeserializeEraser<T>(T);

impl<T> ErasedDeserialize for DeserializeEraser<T>
    where T: Deserialize
{
    type Trait = T::Trait;

    fn deserialize(&self,
                   config: Value,
                   deserializers: &Deserializers)
                   -> Result<Box<Self::Trait>, Box<error::Error + Sync + Send>> {
        let config = try!(config.deserialize_into());
        self.0.deserialize(config, deserializers)
    }
}

struct KeyAdaptor<T: ?Sized>(PhantomData<T>);

impl<T: ?Sized + Any> Key for KeyAdaptor<T> {
    type Value = HashMap<String, Arc<ErasedDeserialize<Trait = T>>>;
}

/// A container of `Deserialize`rs.
#[derive(Clone)]
pub struct Deserializers(ShareCloneMap);

/// Creates a `Deserializers` with the following mappings:
///
/// * Appenders
///     * "console" -> `ConsoleAppenderDeserializer`
///         * Requires the `console_appender` feature (enabled by default).
///     * "file" -> `FileAppenderDeserializer`
///         * Requires the `file_appender` feature (enabled by default).
///     * "rolling_file" -> `RollingFileAppenderDeserializer`
///         * Requires the `rolling_file_appender` feature.
/// * Encoders
///     * "pattern" -> `PatternEncoderDeserializer`
///         * Requires the `pattern_encoder` feature (enabled by default).
///     * "json" -> `JsonEncoderDeserializer`
///         * Requires the `json_encoder` feature.
/// * Filters
///     * "threshold" -> `ThresholdFilterDeserializer`
///         * Requires the `threshold_filter` feature.
/// * Policies
///     *  "compound" -> `CompoundPolicyDeserializer`
///         * Requires the `compound_policy` feature.
/// * Rollers
///     * "delete" -> `DeleteRollerDeserializer`
///         * Requires the `delete_roller` feature.
///     * "fixed_window" -> `FixedWindowRollerDeserializer`
///         * Requires the `fixed_window_roller` feature.
/// * Triggers
///     * "size" -> `SizeTriggerDeserializer`
///         * Requires the `size_trigger` feature.
impl Default for Deserializers {
    fn default() -> Deserializers {
        let mut deserializers = Deserializers::new();

        #[cfg(feature = "console_appender")]
        fn add_console_appender(d: &mut Deserializers) {
            d.insert("console", ::append::console::ConsoleAppenderDeserializer);
        }
        #[cfg(not(feature = "console_appender"))]
        fn add_console_appender(_: &mut Deserializers) {}
        add_console_appender(&mut deserializers);

        #[cfg(feature = "file_appender")]
        fn add_file_appender(d: &mut Deserializers) {
            d.insert("file", ::append::file::FileAppenderDeserializer);
        }
        #[cfg(not(feature = "file_appender"))]
        fn add_file_appender(_: &mut Deserializers) {}
        add_file_appender(&mut deserializers);

        #[cfg(feature = "rolling_file_appender")]
        fn add_rolling_file_appender(d: &mut Deserializers) {
            d.insert("rolling_file",
                     ::append::rolling_file::RollingFileAppenderDeserializer);
        }
        #[cfg(not(feature = "rolling_file_appender"))]
        fn add_rolling_file_appender(_: &mut Deserializers) {}
        add_rolling_file_appender(&mut deserializers);

        #[cfg(feature = "compound_policy")]
        fn add_compound_policy(d: &mut Deserializers) {
            d.insert("compound",
                     ::append::rolling_file::policy::compound::CompoundPolicyDeserializer);
        }
        #[cfg(not(feature = "compound_policy"))]
        fn add_compound_policy(_: &mut Deserializers) {}
        add_compound_policy(&mut deserializers);

        #[cfg(feature = "delete_roller")]
        fn add_delete_roller(d: &mut Deserializers) {
            use append::rolling_file::policy::compound::roll::delete::DeleteRollerDeserializer;
            d.insert("delete", DeleteRollerDeserializer);
        }
        #[cfg(not(feature = "delete_roller"))]
        fn add_delete_roller(_: &mut Deserializers) {}
        add_delete_roller(&mut deserializers);

        #[cfg(feature = "fixed_window_roller")]
        fn add_fixed_window_roller(d: &mut Deserializers) {
            use append::rolling_file::policy::compound::roll::fixed_window;
            d.insert("fixed_window", fixed_window::FixedWindowRollerDeserializer);
        }
        #[cfg(not(feature = "fixed_window_roller"))]
        fn add_fixed_window_roller(_: &mut Deserializers) {}
        add_fixed_window_roller(&mut deserializers);

        #[cfg(feature = "size_trigger")]
        fn add_size_trigger(d: &mut Deserializers) {
            use append::rolling_file::policy::compound::trigger::size::SizeTriggerDeserializer;
            d.insert("size", SizeTriggerDeserializer);
        }
        #[cfg(not(feature = "size_trigger"))]
        fn add_size_trigger(_: &mut Deserializers) {}
        add_size_trigger(&mut deserializers);

        #[cfg(feature = "json_encoder")]
        fn add_json_encoder(d: &mut Deserializers) {
            d.insert("json", ::encode::json::JsonEncoderDeserializer);
        }
        #[cfg(not(feature = "json_encoder"))]
        fn add_json_encoder(_: &mut Deserializers) {}
        add_json_encoder(&mut deserializers);

        #[cfg(feature = "pattern_encoder")]
        fn add_pattern_encoder(d: &mut Deserializers) {
            d.insert("pattern", ::encode::pattern::PatternEncoderDeserializer);
        }
        #[cfg(not(feature = "pattern_encoder"))]
        fn add_pattern_encoder(_: &mut Deserializers) {}
        add_pattern_encoder(&mut deserializers);

        #[cfg(feature = "threshold_filter")]
        fn add_threshold_filter(d: &mut Deserializers) {
            d.insert("threshold",
                     ::filter::threshold::ThresholdFilterDeserializer);
        }
        #[cfg(not(feature = "threshold_filter"))]
        fn add_threshold_filter(_: &mut Deserializers) {}
        add_threshold_filter(&mut deserializers);

        deserializers
    }
}

impl Deserializers {
    /// Creates a new `Deserializers` with no mappings.
    pub fn new() -> Deserializers {
        Deserializers(ShareCloneMap::custom())
    }

    /// Adds a mapping from the specified `kind` to a deserializer.
    pub fn insert<T>(&mut self, kind: &str, deserializer: T)
        where T: Deserialize
    {
        self.0
            .entry::<KeyAdaptor<T::Trait>>()
            .or_insert_with(|| HashMap::new())
            .insert(kind.to_owned(), Arc::new(DeserializeEraser(deserializer)));
    }

    /// Deserializes a value of a specific type and kind.
    pub fn deserialize<T: ?Sized>(&self,
                                  kind: &str,
                                  config: Value)
                                  -> Result<Box<T>, Box<error::Error + Sync + Send>>
        where T: Deserializable
    {
        match self.0.get::<KeyAdaptor<T>>().and_then(|m| m.get(kind)) {
            Some(b) => b.deserialize(config, self),
            None => {
                Err(format!("no {} deserializer for kind `{}` registered",
                            T::name(),
                            kind)
                    .into())
            }
        }
    }
}

/// An error returned when deserializing a configuration into a log4rs `Config`.
#[derive(Debug)]
pub enum Error {
    /// An error deserializing a component.
    Deserialization(Box<error::Error + Sync + Send>),
    /// An error creating the log4rs `Config`.
    Config(config::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::Deserialization(ref err) => {
                write!(fmt, "error deserializing component: {}", err)
            }
            Error::Config(ref err) => write!(fmt, "error creating config: {}", err),
        }
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        "an error deserializing a configuration file into a log4rs `Config`"
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            Error::Deserialization(ref err) => Some(&**err),
            Error::Config(ref err) => Some(err),
        }
    }
}

/// Specifies the format of a configuration file.
#[derive(Copy, Clone)]
pub enum Format {
    /// YAML.
    ///
    /// Requires the `yaml_format` feature (enabled by default).
    #[cfg(feature = "yaml_format")]
    Yaml,

    /// JSON.
    ///
    /// Requires the `json_format` feature.
    #[cfg(feature = "json_format")]
    Json,

    /// TOML.
    ///
    /// Requires the `toml_format` feature.
    #[cfg(feature = "toml_format")]
    Toml,
}

/// A deserialized log4rs configuration file.
pub struct Config {
    refresh_rate: Option<Duration>,
    config: config::Config,
    errors: Vec<Error>,
}

impl Config {
    /// Creates a log4rs `Config` from the specified config string and `Deserializers`.
    pub fn parse(config: &str,
                 format: Format,
                 deserializers: &Deserializers)
                 -> Result<Config, Box<error::Error + Sync + Send>> {
        let mut errors = vec![];

        let config = try!(parse(format, config));

        let raw::Config { refresh_rate,
                          root: raw_root,
                          appenders: raw_appenders,
                          loggers: raw_loggers,
                          .. } = config;

        let root = match raw_root {
            Some(raw_root) => {
                config::Root::builder()
                    .appenders(raw_root.appenders)
                    .build(raw_root.level)
            }
            None => config::Root::builder().build(LogLevelFilter::Debug),
        };

        let mut config = config::Config::builder();

        for (name, raw::Appender { kind, config: raw_config, filters }) in raw_appenders {
            match deserializers.deserialize(&kind, raw_config) {
                Ok(appender_obj) => {
                    let mut builder = config::Appender::builder();
                    for FilterConfig { kind, config } in filters {
                        match deserializers.deserialize(&kind, config) {
                            Ok(filter) => builder = builder.filter(filter),
                            Err(err) => errors.push(Error::Deserialization(err)),
                        }
                    }
                    config = config.appender(builder.build(name.clone(), appender_obj));
                }
                Err(err) => errors.push(Error::Deserialization(err)),
            }
        }

        for (name, logger) in raw_loggers {
            let raw::Logger { level, appenders, additive, .. } = logger;
            let mut logger = config::Logger::builder().appenders(appenders);
            if let Some(additive) = additive {
                logger = logger.additive(additive);
            }
            config = config.logger(logger.build(name, level));
        }

        let (config, config_errors) = config.build_lossy(root);
        if let Err(config_errors) = config_errors {
            for error in config_errors.unpack() {
                errors.push(Error::Config(error));
            }
        }

        let config = Config {
            refresh_rate: refresh_rate.map(|r| r),
            config: config,
            errors: errors,
        };

        Ok(config)
    }

    /// Returns the requested refresh rate.
    pub fn refresh_rate(&self) -> Option<Duration> {
        self.refresh_rate
    }

    /// Returns the log4rs `Config`.
    pub fn into_config(self) -> config::Config {
        self.config
    }

    /// Returns any nonfatal errors encountered when deserializing the config.
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }
}

fn parse(format: Format, _config: &str) -> Result<raw::Config, Box<error::Error + Sync + Send>> {
    match format {
        #[cfg(feature = "yaml_format")]
        Format::Yaml => ::serde_yaml::from_str(_config).map_err(Into::into),
        #[cfg(feature = "json_format")]
        Format::Json => ::serde_json::from_str(_config).map_err(Into::into),
        #[cfg(feature = "toml_format")]
        Format::Toml => {
            use serde::de::Deserialize;

            let mut parser = ::toml::Parser::new(_config);
            let table = match parser.parse() {
                Some(table) => ::toml::Value::Table(table),
                None => return Err(parser.errors.pop().unwrap().into()),
            };
            raw::Config::deserialize(&mut ::toml::Decoder::new(table)).map_err(Into::into)
        }
    }
}

#[cfg(test)]
#[allow(unused_imports)]
mod test {
    use super::*;

    #[test]
    #[cfg(all(feature = "yaml_format", feature = "threshold_filter"))]
    fn full_deserialize() {
        let cfg = r#"
refresh_rate: 60 seconds

appenders:
  console:
    kind: console
    filters:
      - kind: threshold
        level: debug
  baz:
    kind: file
    path: /tmp/baz.log
    encoder:
      pattern: "%m"

root:
  appenders:
    - console
  level: info

loggers:
  foo::bar::baz:
    level: warn
    appenders:
      - baz
    additive: false
"#;
        let config = Config::parse(cfg, Format::Yaml, &Deserializers::default()).unwrap();
        println!("{:?}", config.errors());
        assert!(config.errors().is_empty());
    }

    #[test]
    #[cfg(feature = "yaml_format")]
    fn empty() {
        let config = Config::parse("{}", Format::Yaml, &Deserializers::default()).unwrap();
        assert!(config.errors().is_empty());
    }
}
