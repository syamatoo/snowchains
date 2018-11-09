use crate::errors::{
    ExpandTemplateResult, FileIoError, FileIoErrorCause, FileIoErrorKind, LoadConfigError,
    SuiteFileError, SuiteFileResult,
};
use crate::judging::command::{CompilationCommand, JudgingCommand};
use crate::path::{AbsPath, AbsPathBuf};
use crate::template::Template;
use crate::terminal::WriteAnsi;
use crate::{time, util, yaml};

use itertools::{EitherOrBoth, Itertools as _Itertools};
use maplit::{hashmap, hashset};
use regex::Regex;
use serde::Serialize;
use serde_derive::{Deserialize, Serialize};
use zip::ZipArchive;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::Write as _Write;
use std::iter::FromIterator as _FromIterator;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::{cmp, f64, fmt, str, vec};

pub(crate) fn modify_timelimit(
    stdout: impl WriteAnsi,
    name: &str,
    path: &SuiteFilePath,
    timelimit: Option<Duration>,
) -> SuiteFileResult<()> {
    let mut suite = TestSuite::load(path)?;
    suite.modify_timelimit(&path.path, timelimit)?;
    suite.save(name, path, stdout)
}

pub(crate) fn modify_append(
    name: &str,
    path: &SuiteFilePath,
    input: &str,
    output: Option<&str>,
    stdout: impl WriteAnsi,
) -> SuiteFileResult<()> {
    let mut suite = TestSuite::load(path)?;
    suite.append(input, output)?;
    suite.save(name, path, stdout)
}

pub(crate) fn modify_match(
    stdout: impl WriteAnsi,
    name: &str,
    path: &SuiteFilePath,
    output_match: Match,
) -> SuiteFileResult<()> {
    let mut suite = TestSuite::load(path)?;
    suite.modify_match(output_match)?;
    suite.save(name, path, stdout)
}

/// Extension of a test suite file.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializableExtension {
    Json,
    Toml,
    Yaml,
    Yml,
}

impl Default for SerializableExtension {
    fn default() -> Self {
        SerializableExtension::Yaml
    }
}

impl fmt::Display for SerializableExtension {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SerializableExtension::Json => write!(f, "json"),
            SerializableExtension::Toml => write!(f, "toml"),
            SerializableExtension::Yaml => write!(f, "yaml"),
            SerializableExtension::Yml => write!(f, "yml"),
        }
    }
}

impl FromStr for SerializableExtension {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "json" => Ok(SerializableExtension::Json),
            "toml" => Ok(SerializableExtension::Toml),
            "yaml" => Ok(SerializableExtension::Yaml),
            "yml" => Ok(SerializableExtension::Yml),
            s => Err(format!("Unsupported extension: {:?}", s)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuiteFileExtension {
    Json,
    Toml,
    Yaml,
    Yml,
    Zip,
}

impl SuiteFileExtension {
    fn serializable(self) -> Option<SerializableExtension> {
        match self {
            SuiteFileExtension::Json => Some(SerializableExtension::Json),
            SuiteFileExtension::Toml => Some(SerializableExtension::Toml),
            SuiteFileExtension::Yaml => Some(SerializableExtension::Yaml),
            SuiteFileExtension::Yml => Some(SerializableExtension::Yml),
            SuiteFileExtension::Zip => None,
        }
    }
}

impl FromStr for SuiteFileExtension {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "json" => Ok(SuiteFileExtension::Json),
            "toml" => Ok(SuiteFileExtension::Toml),
            "yaml" => Ok(SuiteFileExtension::Yaml),
            "yml" => Ok(SuiteFileExtension::Yml),
            "zip" => Ok(SuiteFileExtension::Zip),
            _ => Err(format!("Unsupported extension: {:?}", s)),
        }
    }
}

impl fmt::Display for SuiteFileExtension {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            SuiteFileExtension::Json => write!(f, "json"),
            SuiteFileExtension::Toml => write!(f, "toml"),
            SuiteFileExtension::Yaml => write!(f, "yaml"),
            SuiteFileExtension::Yml => write!(f, "yml"),
            SuiteFileExtension::Zip => write!(f, "zip"),
        }
    }
}

pub(crate) struct TestCaseLoader<'a> {
    template: Template<AbsPathBuf>,
    extensions: &'a BTreeSet<SuiteFileExtension>,
    zip_conf: &'a ZipConfig,
    tester_compilacions: HashMap<String, Template<CompilationCommand>>,
    tester_commands: HashMap<String, Template<JudgingCommand>>,
}

impl<'a> TestCaseLoader<'a> {
    pub fn new(
        template: Template<AbsPathBuf>,
        extensions: &'a BTreeSet<SuiteFileExtension>,
        zip_conf: &'a ZipConfig,
        tester_compilacions: HashMap<String, Template<CompilationCommand>>,
        tester_commands: HashMap<String, Template<JudgingCommand>>,
    ) -> Self {
        Self {
            template,
            extensions,
            zip_conf,
            tester_compilacions,
            tester_commands,
        }
    }

    pub fn load_merging(&self, problem: &str) -> SuiteFileResult<(TestCases, String)> {
        fn format_paths(paths: &[impl AsRef<str>]) -> String {
            let mut pref_common = "".to_owned();
            let mut suf_common_rev = vec![];
            let mut css = paths
                .iter()
                .map(|s| s.as_ref().chars().collect::<VecDeque<_>>())
                .collect::<Vec<_>>();
            while same_nexts(&css, VecDeque::front) {
                let mut css = css.iter_mut();
                pref_common.extend(css.next().and_then(|cs| cs.pop_front()));
                for cs in css {
                    cs.pop_front();
                }
            }
            while same_nexts(&css, VecDeque::back) {
                let mut css = css.iter_mut().rev();
                suf_common_rev.extend(css.next().and_then(|cs| cs.pop_back()));
                for cs in css {
                    cs.pop_back();
                }
            }
            let mut outcome = pref_common;
            let css = css
                .into_iter()
                .map(|cs| cs.into_iter().collect::<String>())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            let n = css.len();
            for (i, s) in css.into_iter().enumerate() {
                match i {
                    0 => outcome.push('{'),
                    _ => outcome.push_str(", "),
                }
                outcome.push_str(&s);
                if i == n - 1 {
                    outcome.push('}');
                }
            }
            outcome.extend(suf_common_rev.into_iter().rev());
            outcome
        }

        fn same_nexts<T: PartialEq + Copy>(
            xss: &[VecDeque<T>],
            f: fn(&VecDeque<T>) -> Option<&T>,
        ) -> bool {
            !xss.is_empty() && xss.iter().all(|xs| f(xs).is_some()) && {
                let mut xss = xss.iter();
                let x0 = f(xss.next().unwrap()).unwrap();
                xss.all(|xs| f(xs).unwrap() == x0)
            }
        }

        let all_paths = self
            .extensions
            .iter()
            .map(|ext| {
                self.template
                    .clone()
                    .insert_string("extension", ext.to_string())
                    .expand(problem)
                    .map(|path| (path, ext))
            }).collect::<ExpandTemplateResult<Vec<_>>>()?;

        let existing_paths = all_paths
            .iter()
            .cloned()
            .filter(|(path, _)| path.exists())
            .collect::<Vec<_>>();

        let mut simple_cases = vec![];
        let mut interactive_cases = vec![];
        let mut filepaths = vec![];

        for (path, extension) in existing_paths {
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if let Some(extension) = extension.serializable() {
                let path = SuiteFilePath { path, extension };
                match TestSuite::load(&path)? {
                    TestSuite::Simple(suite) => simple_cases.extend(suite.cases_named(&filename)),
                    TestSuite::Interactive(suite) => {
                        interactive_cases.extend(suite.cases_named(
                            &self.tester_compilacions,
                            &self.tester_commands,
                            &filename,
                            problem,
                        )?);
                    }
                    TestSuite::Unsubmittable => {
                        return Err(SuiteFileError::Unsubmittable(problem.to_owned()))
                    }
                }
                filepaths.push(path.path.display().to_string());
            } else {
                let cases = self.zip_conf.load(&path, &filename)?;
                if !cases.is_empty() {
                    simple_cases.extend(cases);
                    filepaths.push(path.display().to_string());
                }
            }
        }

        let paths_as_text = format_paths(&filepaths);

        if simple_cases.is_empty() && interactive_cases.is_empty() {
            let all_paths = all_paths
                .into_iter()
                .map(|(path, _)| path.display().to_string())
                .collect::<Vec<_>>();
            Err(SuiteFileError::NoFile(format_paths(&all_paths)))
        } else if interactive_cases.is_empty() {
            Ok((TestCases::Simple(simple_cases), paths_as_text))
        } else if simple_cases.is_empty() {
            Ok((TestCases::Interactive(interactive_cases), paths_as_text))
        } else {
            Err(SuiteFileError::DifferentTypesOfSuites)
        }
    }
}

pub(crate) struct DownloadDestinations {
    template: Template<AbsPathBuf>,
    scraping_ext: SerializableExtension,
}

impl DownloadDestinations {
    pub fn new(template: Template<AbsPathBuf>, scraping_ext: SerializableExtension) -> Self {
        Self {
            template,
            scraping_ext,
        }
    }

    pub fn scraping(&self, problem: &str) -> ExpandTemplateResult<SuiteFilePath> {
        let path = self
            .template
            .clone()
            .insert_string("extension", self.scraping_ext.to_string())
            .expand(problem)?;
        Ok(SuiteFilePath::new(&path, self.scraping_ext))
    }

    pub fn zip(&self, problem: &str) -> ExpandTemplateResult<AbsPathBuf> {
        self.template
            .clone()
            .insert_string("extension", "zip")
            .expand(problem)
    }
}

/// File path which extension is 'json', 'toml', 'yaml', or 'yml'.
pub(crate) struct SuiteFilePath {
    path: AbsPathBuf,
    extension: SerializableExtension,
}

impl SuiteFilePath {
    pub fn new(path: &AbsPath, extension: SerializableExtension) -> Self {
        let path = path.to_owned();
        Self { path, extension }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct ZipConfig {
    #[serde(
        serialize_with = "time::ser_millis",
        deserialize_with = "time::de_secs",
        skip_serializing_if = "Option::is_none",
    )]
    timelimit: Option<Duration>,
    #[serde(rename = "match")]
    output_match: Match,
    entries: Vec<ZipEntries>,
}

impl ZipConfig {
    fn load(&self, path: &AbsPath, filename: &str) -> SuiteFileResult<Vec<SimpleCase>> {
        let mut cases = vec![];
        for entry in &self.entries {
            cases.extend(entry.load(path, filename, self.timelimit, self.output_match)?);
        }
        Ok(cases)
    }
}

#[derive(Serialize, Deserialize)]
struct ZipEntries {
    sort: Vec<ZipEntriesSorting>,
    #[serde(rename = "in")]
    input: ZipEntry,
    #[serde(rename = "out")]
    output: ZipEntry,
}

impl ZipEntries {
    fn load(
        &self,
        path: &AbsPath,
        filename: &str,
        timelimit: Option<Duration>,
        output_match: Match,
    ) -> SuiteFileResult<Vec<SimpleCase>> {
        if !path.exists() {
            return Ok(vec![]);
        }
        let mut zip =
            ZipArchive::new(crate::fs::open(path)?).map_err(|e| FileIoError::read_zip(path, e))?;
        let mut pairs = hashmap!();
        for i in 0..zip.len() {
            let (filename, content) = {
                let file = zip
                    .by_index(i)
                    .map_err(|e| FileIoError::read_zip(path, e))?;
                let filename = file.name().to_owned();
                let content = util::string_from_read(file, 0)
                    .map_err(|e| FileIoError::read_zip(path, e.into()))?;
                (filename, content)
            };
            if let Some(caps) = self.input.entry.captures(&filename) {
                let name = caps
                    .get(self.input.match_group)
                    .ok_or_else(|| SuiteFileError::RegexGroupOutOfBounds(self.input.match_group))?
                    .as_str()
                    .to_owned();
                let content = if self.input.crlf_to_lf && content.contains("\r\n") {
                    content.replace("\r\n", "\n")
                } else {
                    content.clone()
                };
                if let Some((_, output)) = pairs.remove(&name) {
                    pairs.insert(name, (Some(content), output));
                } else {
                    pairs.insert(name, (Some(content), None));
                }
            }
            if let Some(caps) = self.output.entry.captures(&filename) {
                let name = caps
                    .get(self.output.match_group)
                    .ok_or_else(|| SuiteFileError::RegexGroupOutOfBounds(self.output.match_group))?
                    .as_str()
                    .to_owned();
                let content = if self.output.crlf_to_lf && content.contains("\r\n") {
                    content.replace("\r\n", "\n")
                } else {
                    content
                };
                if let Some((input, _)) = pairs.remove(&name) {
                    pairs.insert(name, (input, Some(content)));
                } else {
                    pairs.insert(name, (None, Some(content)));
                }
            }
        }
        let mut cases = pairs
            .into_iter()
            .filter_map(|(name, (input, output))| {
                if let (Some(input), Some(output)) = (input, output) {
                    Some((name, input, output))
                } else {
                    None
                }
            }).collect::<Vec<_>>();
        for sorting in &self.sort {
            match sorting {
                ZipEntriesSorting::Dictionary => cases.sort_by(|(s1, _, _), (s2, _, _)| s1.cmp(s2)),
                ZipEntriesSorting::Number => cases.sort_by(|(s1, _, _), (s2, _, _)| {
                    match (s1.parse::<usize>(), s2.parse::<usize>()) {
                        (Ok(n1), Ok(n2)) => n1.cmp(&n2),
                        (Ok(_), Err(_)) => cmp::Ordering::Less,
                        (Err(_), Ok(_)) => cmp::Ordering::Greater,
                        (Err(_), Err(_)) => cmp::Ordering::Equal,
                    }
                }),
            }
        }
        let cases = cases
            .into_iter()
            .map(|(name, input, output)| {
                SimpleCase::new(
                    &format!("{}:{}", filename, name),
                    timelimit,
                    &input,
                    Some(&output),
                    output_match,
                )
            }).collect();
        Ok(cases)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ZipEntriesSorting {
    Dictionary,
    Number,
}

#[derive(Serialize, Deserialize)]
struct ZipEntry {
    #[serde(
        serialize_with = "yaml::serialize_regex",
        deserialize_with = "yaml::deserialize_regex"
    )]
    entry: Regex,
    match_group: usize,
    crlf_to_lf: bool,
}

/// `SimpelSuite` or `InteractiveSuite`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TestSuite {
    Simple(SimpleSuite),
    Interactive(InteractiveSuite),
    Unsubmittable,
}

impl TestSuite {
    fn load(path: &SuiteFilePath) -> SuiteFileResult<Self> {
        fn chain_err<E: Into<FileIoErrorCause>>(err: E, path: &Path) -> FileIoError {
            FileIoError::new(FileIoErrorKind::Deserialize, path).with(err)
        }

        let (path, extension) = (&path.path, path.extension);
        let content = crate::fs::read_to_string(&path)?;
        match extension {
            SerializableExtension::Json => {
                serde_json::from_str(&content).map_err(|e| chain_err(e, &path))
            }
            SerializableExtension::Toml => {
                toml::from_str(&content).map_err(|e| chain_err(e, &path))
            }
            SerializableExtension::Yaml | SerializableExtension::Yml => {
                serde_yaml::from_str(&content).map_err(|e| chain_err(e, &path))
            }
        }.map_err(Into::into)
    }

    /// Serializes `self` and save it to given path.
    pub fn save(
        &self,
        name: &str,
        path: &SuiteFilePath,
        mut out: impl WriteAnsi,
    ) -> SuiteFileResult<()> {
        let (path, extension) = (&path.path, path.extension);
        let serialized = self.to_string_pretty(extension)?;
        crate::fs::write(path, serialized.as_bytes())?;
        out.with_reset(|o| o.bold()?.write_str(name))?;
        write!(out, ": Saved to {} ", path.display())?;
        match self {
            TestSuite::Simple(s) => match s.cases.len() {
                0 => out.with_reset(|o| o.fg(11)?.write_str("(no test case)\n")),
                1 => out.with_reset(|o| o.fg(10)?.write_str("(1 test case)\n")),
                n => out.with_reset(|o| writeln!(o.fg(10)?, "({} test cases)", n)),
            },
            TestSuite::Interactive(_) => {
                out.with_reset(|o| o.fg(10)?.write_str("(interactive problem)\n"))
            }
            TestSuite::Unsubmittable => {
                out.with_reset(|o| o.fg(10)?.write_str("(unsubmittable problem)\n"))
            }
        }.map_err(Into::into)
    }

    fn to_string_pretty(&self, ext: SerializableExtension) -> SuiteFileResult<String> {
        match self {
            TestSuite::Simple(this) => this.to_string_pretty(ext),
            TestSuite::Interactive(_) | TestSuite::Unsubmittable => match ext {
                SerializableExtension::Json => {
                    serde_json::to_string_pretty(self).map_err(Into::into)
                }
                SerializableExtension::Toml => toml::to_string_pretty(self).map_err(Into::into),
                SerializableExtension::Yaml | SerializableExtension::Yml => {
                    serde_yaml::to_string(self).map_err(Into::into)
                }
            },
        }
    }

    fn modify_timelimit(
        &mut self,
        path: &Path,
        timelimit: Option<Duration>,
    ) -> SuiteFileResult<()> {
        match self {
            TestSuite::Simple(suite) => {
                suite.head.timelimit = timelimit;
                Ok(())
            }
            TestSuite::Interactive(suite) => {
                suite.timelimit = timelimit;
                Ok(())
            }
            TestSuite::Unsubmittable => {
                Err(SuiteFileError::Unsubmittable(path.display().to_string()))
            }
        }
    }

    fn append(&mut self, input: &str, output: Option<&str>) -> SuiteFileResult<()> {
        match self {
            TestSuite::Simple(suite) => {
                suite.append(input, output);
                Ok(())
            }
            _ => Err(SuiteFileError::SuiteIsNotSimple),
        }
    }

    fn modify_match(&mut self, output_match: Match) -> SuiteFileResult<()> {
        match self {
            TestSuite::Simple(suite) => {
                suite.head.output_match = output_match;
                Ok(())
            }
            _ => Err(SuiteFileError::SuiteIsNotSimple),
        }
    }
}

impl From<SimpleSuite> for TestSuite {
    fn from(from: SimpleSuite) -> Self {
        TestSuite::Simple(from)
    }
}

impl From<InteractiveSuite> for TestSuite {
    fn from(from: InteractiveSuite) -> Self {
        TestSuite::Interactive(from)
    }
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SimpleSuite {
    #[serde(flatten)]
    head: SimpleSuiteSchemaHead,
    cases: Vec<SimpleSuiteSchemaCase>,
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
struct SimpleSuiteSchemaHead {
    #[serde(
        serialize_with = "time::ser_millis",
        deserialize_with = "time::de_secs",
        skip_serializing_if = "Option::is_none",
    )]
    timelimit: Option<Duration>,
    #[serde(rename = "match", default)]
    output_match: Match,
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SimpleSuiteSchemaCase {
    #[serde(rename = "in")]
    input: String,
    #[serde(rename = "out", skip_serializing_if = "Option::is_none")]
    output: Option<String>,
}

impl SimpleSuite {
    pub(crate) fn new(timelimit: impl Into<Option<Duration>>) -> Self {
        Self {
            head: SimpleSuiteSchemaHead {
                timelimit: timelimit.into(),
                output_match: Match::default(),
            },
            cases: vec![],
        }
    }

    pub(crate) fn any(mut self) -> Self {
        self.head.output_match = Match::Any;
        self
    }

    pub(crate) fn cases<S: Into<String>, O: Into<Option<S>>, I: IntoIterator<Item = (S, O)>>(
        mut self,
        cases: I,
    ) -> Self {
        self.cases.extend(
            cases
                .into_iter()
                .map(|(input, output)| SimpleSuiteSchemaCase {
                    input: input.into(),
                    output: output.into().map(Into::into),
                }),
        );
        self
    }

    fn cases_named(self, filename: &str) -> Vec<SimpleCase> {
        let (output_match, timelimit) = (self.head.output_match, self.head.timelimit);
        self.cases
            .into_iter()
            .enumerate()
            .map(move |(i, case)| {
                SimpleCase::new(
                    &format!("{}[{}]", filename, i),
                    timelimit,
                    &case.input,
                    case.output.as_ref().map(String::as_str),
                    output_match,
                )
            }).collect::<Vec<_>>()
    }

    fn to_string_pretty(&self, ext: SerializableExtension) -> SuiteFileResult<String> {
        #[derive(Serialize)]
        struct WithType<T: Serialize> {
            r#type: &'static str,
            #[serde(flatten)]
            repr: T,
        }

        impl<T: Serialize> WithType<T> {
            fn new(repr: T) -> Self {
                Self {
                    r#type: "simple",
                    repr,
                }
            }
        }

        fn is_valid(s: &str) -> bool {
            s.ends_with('\n') && s
                .chars()
                .all(|c| [' ', '\n'].contains(&c) || (!c.is_whitespace() && !c.is_control()))
        }

        match ext {
            SerializableExtension::Json => {
                serde_json::to_string_pretty(&WithType::new(self)).map_err(Into::into)
            }
            SerializableExtension::Toml => {
                toml::to_string_pretty(&WithType::new(self)).map_err(Into::into)
            }
            SerializableExtension::Yaml | SerializableExtension::Yml => {
                let mut r = serde_yaml::to_string(&WithType::new(self))?;
                let cases = &self.cases;
                let all_valid = cases.iter().all(|SimpleSuiteSchemaCase { input, output }| {
                    is_valid(input) && output.as_ref().map_or(true, |o| is_valid(o))
                });
                if all_valid {
                    r = serde_yaml::to_string(&WithType::new(&self.head))?;
                    r += "\ncases:\n";
                    for SimpleSuiteSchemaCase { input, output } in cases {
                        r += "  - in: |\n";
                        for l in input.lines() {
                            writeln!(r, "      {}", l).unwrap();
                        }
                        if let Some(output) = output {
                            r += "    out: |\n";
                            for l in output.lines() {
                                writeln!(r, "      {}", l).unwrap();
                            }
                        }
                    }
                }
                Ok(r)
            }
        }
    }

    fn append(&mut self, input: &str, output: Option<&str>) {
        self.cases.push(SimpleSuiteSchemaCase {
            input: input.to_owned(),
            output: output.map(ToOwned::to_owned),
        });
    }
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct InteractiveSuite {
    #[serde(
        serialize_with = "time::ser_millis",
        deserialize_with = "time::de_secs",
        skip_serializing_if = "Option::is_none",
    )]
    timelimit: Option<Duration>,
    tester: Option<String>,
    each_args: Vec<Vec<String>>,
}

impl InteractiveSuite {
    pub fn new(timelimit: impl Into<Option<Duration>>) -> Self {
        Self {
            timelimit: timelimit.into(),
            tester: None,
            each_args: vec![],
        }
    }

    fn cases_named(
        &self,
        tester_compilations: &HashMap<String, Template<CompilationCommand>>,
        tester_commands: &HashMap<String, Template<JudgingCommand>>,
        filename: &str,
        problem: &str,
    ) -> SuiteFileResult<vec::IntoIter<InteractiveCase>> {
        let mut cases = Vec::with_capacity(self.each_args.len());
        for (i, args) in self.each_args.iter().enumerate() {
            let lang = self
                .tester
                .as_ref()
                .ok_or_else(|| LoadConfigError::LanguageNotSpecified)?;
            let mut m = hashmap!("*".to_owned() => args.join(" "));
            m.extend(args.iter().enumerate().zip_longest(1..=9).map(|p| match p {
                EitherOrBoth::Both((_, arg), i) => (i.to_string(), arg.clone()),
                EitherOrBoth::Left((j, arg)) => ((j + i).to_string(), arg.clone()),
                EitherOrBoth::Right(i) => (i.to_string(), "".to_owned()),
            }));
            let tester_compilation = match tester_compilations
                .get(lang)
                .map(|t| t.clone().expand(&problem))
            {
                None => Ok(None),
                Some(Err(err)) => Err(err),
                Some(Ok(comp)) => Ok(Some(Arc::new(comp))),
            }?;
            let tester = tester_commands
                .get(lang)
                .map(|template| template.clone().clone())
                .ok_or_else(|| LoadConfigError::NoSuchLanguage(lang.to_owned()))?
                .insert_strings(&m)
                .expand(&problem)?;
            cases.push(InteractiveCase {
                name: Arc::new(format!("{}[{}]", filename, i)),
                tester: Arc::new(tester),
                tester_compilation,
                timelimit: self.timelimit,
            });
        }
        Ok(cases.into_iter())
    }
}

/// `Vec<SimpleCase>` or `Vec<ReducibleCase>`.
pub(crate) enum TestCases {
    Simple(Vec<SimpleCase>),
    Interactive(Vec<InteractiveCase>),
}

impl TestCases {
    pub fn interactive_tester_compilations(&self) -> HashSet<Arc<CompilationCommand>> {
        match self {
            TestCases::Simple(_) => hashset!(),
            TestCases::Interactive(cases) => {
                let compilations = cases
                    .iter()
                    .filter_map(|case| case.tester_compilation.clone());
                HashSet::from_iter(compilations)
            }
        }
    }
}

pub(crate) trait TestCase {
    /// Gets `name`.
    fn name(&self) -> Arc<String>;
}

/// Pair of `input` and `expected`.
#[derive(Clone)]
pub(crate) struct SimpleCase {
    name: Arc<String>,
    input: Arc<String>,
    expected: Arc<ExpectedStdout>,
    timelimit: Option<Duration>,
}

impl TestCase for SimpleCase {
    fn name(&self) -> Arc<String> {
        self.name.clone()
    }
}

impl SimpleCase {
    fn new(
        name: &str,
        timelimit: Option<Duration>,
        input: &str,
        output: Option<&str>,
        output_match: Match,
    ) -> Self {
        let expected = match (output_match, output.map(ToOwned::to_owned)) {
            (Match::Any, example) => ExpectedStdout::Any { example },
            (Match::Exact, None) | (Match::Float { .. }, None) => {
                ExpectedStdout::Any { example: None }
            }
            (Match::Exact, Some(output)) => ExpectedStdout::Exact(output),
            (
                Match::Float {
                    absolute_error,
                    relative_error,
                },
                Some(string),
            ) => ExpectedStdout::Float {
                string,
                absolute_error,
                relative_error,
            },
        };
        Self {
            name: Arc::new(name.to_owned()),
            input: Arc::new(input.to_owned()),
            expected: Arc::new(expected),
            timelimit,
        }
    }

    pub(crate) fn input(&self) -> Arc<String> {
        self.input.clone()
    }

    pub(crate) fn expected(&self) -> Arc<ExpectedStdout> {
        self.expected.clone()
    }

    pub(crate) fn timelimit(&self) -> Option<Duration> {
        self.timelimit
    }
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(crate) enum ExpectedStdout {
    Any {
        example: Option<String>,
    },
    Exact(String),
    Float {
        string: String,
        absolute_error: f64,
        relative_error: f64,
    },
}

#[cfg_attr(test, derive(PartialEq))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Match {
    Any,
    Exact,
    Float {
        #[serde(default = "nan")]
        relative_error: f64,
        #[serde(default = "nan")]
        absolute_error: f64,
    },
}

fn nan() -> f64 {
    f64::NAN
}

impl Default for Match {
    fn default() -> Self {
        Match::Exact
    }
}

#[cfg_attr(test, derive(Debug, PartialEq))]
#[derive(Clone)]
pub(crate) struct InteractiveCase {
    name: Arc<String>,
    timelimit: Option<Duration>,
    tester: Arc<JudgingCommand>,
    tester_compilation: Option<Arc<CompilationCommand>>,
}

impl TestCase for InteractiveCase {
    fn name(&self) -> Arc<String> {
        self.name.clone()
    }
}

impl InteractiveCase {
    pub fn tester(&self) -> Arc<JudgingCommand> {
        self.tester.clone()
    }

    pub fn timelimit(&self) -> Option<Duration> {
        self.timelimit
    }
}
