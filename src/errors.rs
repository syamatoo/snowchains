use {bincode, cookie, httpsession, regex, serde_json, serde_urlencoded, serde_yaml, toml};
use chrono::{self, DateTime, Local};
use httpsession::UrlError;
use zip::result::ZipError;

use std::{self, fmt, io};
use std::process::ExitStatus;
use std::string::FromUtf8Error;
use std::sync::mpsc::RecvError;

error_chain!{
    links {
        Service(ServiceError, ServiceErrorKind);
        Judge(JudgeError, JudgeErrorKind);
        SuiteFile(SuiteFileError, SuiteFileErrorKind);
        Config(ConfigError, ConfigErrorKind);
    }
}

error_chain! {
    types {
        ServiceError, ServiceErrorKind, ServiceResultExt, ServiceResult;
    }

    links {
        CodeReplace(CodeReplaceError, CodeReplaceErrorKind);
        SuiteFile(SuiteFileError, SuiteFileErrorKind);
    }

    foreign_links {
        Bincode(bincode::Error);
        ChronoParse(chrono::ParseError);
        CookieParse(cookie::ParseError);
        HttpSession(httpsession::Error);
        Io(io::Error);
        Recv(RecvError);
        SerdeJson(serde_json::Error);
        SerdeUrlencodedSer(serde_urlencoded::ser::Error);
        Url(UrlError);
        Zip(ZipError);
    }

    errors {
        AlreadyAccepted {
            description("Found an accepted submission")
            display("Found an accepted submission. Add \"--force\" to submit")
        }

        ContestNotBegun(contest_name: String, begins_at: DateTime<Local>) {
            description("Contest has not begun yet")
            display("{} will begin at {}", contest_name, begins_at)
        }

        ContestNotFound(contest_name: String) {
            description("Contest not found")
            display("{} not found", contest_name)
        }

        NoSuchProblem(name: String) {
            description("No such problem")
            display("No such problem: {:?}", name)
        }

        Scrape {
            description("Scraping failed")
            display("Scraping failed")
        }

        Thread {
            description("Thread error")
            display("Thread error")
        }

        Webbrowser(status: ExitStatus) {
            description("Failed to open a URL in the default browser")
            display("The default browser terminated abnormally {}",
                    if let Some(code) = status.code() { format!("with code {}", code) }
                    else { "without code".to_owned() })
        }
    }
}

error_chain! {
    types {
        JudgeError, JudgeErrorKind, JudgeResultExt, JudgeResult;
    }

    links {
        SuiteFile(SuiteFileError, SuiteFileErrorKind);
    }

    foreign_links {
        Io(io::Error);
        Recv(RecvError);
    }

    errors {
        CommandNotFound(command: String) {
            description("Command not found")
            display("No such command: {:?}", command)
        }

        Compile(status: ExitStatus) {
            description("Compilation failed")
            display("The compilation command terminated abnormally {}",
                    if let Some(code) = status.code() { format!("with code {}", code) }
                    else { "without code".to_owned() })
        }

        TestFailure(n: usize, d: usize) {
            description("Test failed")
            display("{}/{} Test{} failed", n, d, if *n > 0 { "s" } else { "" })
        }
    }
}

error_chain! {
    types {
        SuiteFileError, SuiteFileErrorKind, SuiteFileResultExt, SuiteFileResult;
    }

    foreign_links {
        Io(io::Error);
        SerdeJson(serde_json::Error);
        SerdeYaml(serde_yaml::Error);
        TomlDe(toml::de::Error);
        TomlSer(toml::ser::Error);
    }

    errors {
        DifferentTypesOfSuites {
            description("Different types of suites")
            display("Different types of suites")
        }

        SuiteIsInteractive {
            description("Target suite is interactive")
            display("Target suite is interactive")
        }
    }
}

error_chain! {
    types {
        ConfigError, ConfigErrorKind, ConfigResultExt, ConfigResult;
    }

    links {
        CodeReplace(CodeReplaceError, CodeReplaceErrorKind);
    }

    foreign_links {
        Io(io::Error);
        Regex(regex::Error);
        SerdeYaml(serde_yaml::Error);
        Template(TemplateError);
    }

    errors {
        ConfigFileNotFound {
            description("\"snowchains.yaml\" not found")
            display("\"snowchains.yaml\" not found")
        }

        LanguageNotSpecified {
            description("Language not specified")
            display("Language not specified")
        }

        NoSuchLanguage(name: String) {
            description("Language not found")
            display("No such language: \"{}\"", name)
        }

        PropertyNotSet(property: &'static str) {
            description("Property not set")
            display("Property not set: \"{}\"", property)
        }
    }
}

error_chain! {
    types {
        CodeReplaceError, CodeReplaceErrorKind, CodeReplaceResultExt, CodeReplaceResult;
    }

    foreign_links {
        Regex(regex::Error);
        Template(TemplateError);
        FromUtf8(FromUtf8Error);
    }

    errors {
        RegexGroupOutOfBounds(i: usize) {
            description("Regex group out of bounds")
            display("Regex group out of bounds: {}", i)
        }

        NoMatch(regex: String) {
            description("No match")
            display("No match: {:?}", regex)
        }
    }
}

pub type TemplateResult<T> = std::result::Result<T, TemplateError>;

#[derive(Debug)]
pub enum TemplateError {
    InvalidVariable(String),
    Syntax(String),
    NoSuchSpecifier(String, String, &'static [&'static str]),
    NoSuchVariable(String, String, Vec<String>),
    NonUtf8EnvVar(String),
    Io(io::Error),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TemplateError::InvalidVariable(ref s) => write!(f, "Invalid variable: {:?}", s),
            TemplateError::Syntax(ref s) => write!(f, "Syntax error: {:?}", s),
            TemplateError::NoSuchSpecifier(ref s, ref specifier, expected) => write!(
                f,
                "No such format specifier {:?} (expected {:?}): {:?}",
                specifier, expected, s
            ),
            TemplateError::NoSuchVariable(ref s, ref keyword, ref expected) => write!(
                f,
                "No such variable {:?} (expected {:?} + environment variables): {:?}",
                keyword, expected, s
            ),
            TemplateError::NonUtf8EnvVar(ref k) => {
                write!(f, "Non UTF-8 environment variable: {:?}", k)
            }
            TemplateError::Io(ref e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for TemplateError {
    fn description(&self) -> &str {
        "Error about format string in config file"
    }

    fn cause(&self) -> Option<&std::error::Error> {
        match *self {
            TemplateError::Io(ref e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for TemplateError {
    fn from(from: io::Error) -> Self {
        TemplateError::Io(from)
    }
}
