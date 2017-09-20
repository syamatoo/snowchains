use error::{PrintChainColored, ServiceErrorKind, ServiceResult};
use service::scraping_session::ScrapingSession;
use testcase::{Cases, TestCaseFileExtension, TestCaseFilePath};

use regex::Regex;
use reqwest::StatusCode;
use select::document::Document;
use select::node::Node;
use select::predicate::{And, Attr, Class, Name, Predicate, Text};
use std::io::Read;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use webbrowser;


/// Logins to "beta.atcoder.jp".
pub fn login() -> ServiceResult<()> {
    AtCoderBeta::load_or_login(Some("Already signed in."))?
        .save()
}


/// Participates in given contest.
pub fn participate(contest_name: &str) -> ServiceResult<()> {
    let contest = &Contest::new(contest_name)?;
    let mut atcoder = AtCoderBeta::load_or_login(None)?;
    atcoder.register_to_contest(contest)?;
    atcoder.save()
}


/// Access to pages of the problems and extract pairs of sample input/output from them.
pub fn download(
    contest_name: &str,
    dir_to_save: &Path,
    extension: TestCaseFileExtension,
    open_browser: bool,
) -> ServiceResult<()> {
    let contest = Contest::new(contest_name)?;
    let mut atcoder = AtCoderBeta::load_or_login(None)?;
    atcoder.register_to_contest(&contest)?;
    atcoder.download_all_tasks(
        contest,
        dir_to_save,
        extension,
        open_browser,
    )?;
    atcoder.save()
}


/// Submits a source code.
pub fn submit(
    contest_name: &str,
    task: &str,
    lang_id: u32,
    src_path: &Path,
    open_browser: bool,
) -> ServiceResult<()> {
    println!("");
    let contest = Contest::new(contest_name)?;
    let mut atcoder = AtCoderBeta::load_or_login(None)?;
    atcoder.register_to_contest(&contest)?;
    atcoder.submit_code(contest, task, lang_id, src_path, open_browser)
}


struct AtCoderBeta(ScrapingSession);

impl Deref for AtCoderBeta {
    type Target = ScrapingSession;
    fn deref(&self) -> &ScrapingSession {
        &self.0
    }
}

impl DerefMut for AtCoderBeta {
    fn deref_mut(&mut self) -> &mut ScrapingSession {
        &mut self.0
    }
}

impl AtCoderBeta {
    fn load_or_login(message: Option<&'static str>) -> ServiceResult<Self> {
        let mut atcoder = AtCoderBeta(ScrapingSession::from_db("atcoder-beta.sqlite3")?);
        if !atcoder.no_cookie() && atcoder.http_get("https://beta.atcoder.jp/settings").is_ok() {
            if let Some(message) = message {
                println!("{}", message);
            }
        } else {
            atcoder.login()?;
        }
        Ok(atcoder)
    }

    fn login(&mut self) -> ServiceResult<()> {
        while let Err(e) = self.try_logging_in() {
            e.print_chain_colored();
            eprintln!("Failed to login. Try again.");
            self.clear_cookies();
        }
        Ok(())
    }

    fn try_logging_in(&mut self) -> ServiceResult<()> {
        #[derive(Serialize)]
        struct PostData {
            username: String,
            password: String,
            csrf_token: String,
        }

        let csrf_token = extract_csrf_token(self.http_get(URL)?)?;
        let (username, password) = super::read_username_and_password("Username: ")?;
        let data = PostData {
            username: username,
            password: password,
            csrf_token: csrf_token,
        };
        static URL: &'static str = "https://beta.atcoder.jp/login";
        let _ = self.http_post_urlencoded(URL, data, StatusCode::Found)?;
        let _ = self.http_get("https://beta.atcoder.jp/settings")?;
        Ok(println!("Succeeded to login."))
    }

    fn register_to_contest(&mut self, contest: &Contest) -> ServiceResult<()> {
        #[derive(Serialize)]
        struct PostData {
            csrf_token: String,
        }

        let token =
            PostData { csrf_token: extract_csrf_token(self.http_get(&contest.top_url())?)? };
        self.http_post_urlencoded(&contest.registration_url(), token, StatusCode::Found)
            .map(|_| ())
    }

    fn download_all_tasks(
        &mut self,
        contest: Contest,
        dir_to_save: &Path,
        extension: TestCaseFileExtension,
        open_browser: bool,
    ) -> ServiceResult<()> {
        let mut outputs = vec![];
        let urls_with_names = extract_task_urls_with_names(self.http_get(&contest.tasks_url())?)?;
        for (name, relative_url) in urls_with_names {
            let url = format!("https://beta.atcoder.jp{}", relative_url);
            let cases = extract_cases(self.http_get(&url)?, &contest.style())?;
            let path = TestCaseFilePath::new(&dir_to_save, &name.to_lowercase(), extension);
            outputs.push((url, cases, path));
        }
        for &(_, ref cases, ref path) in &outputs {
            cases.save(&path)?;
        }
        if open_browser {
            for (url, ..) in outputs.into_iter() {
                println!("Opening {} in default browser...", url);
                webbrowser::open(&url)?;
            }
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    fn submit_code(
        &mut self,
        contest: Contest,
        task: &str,
        lang_id: u32,
        src_path: &Path,
        open_browser: bool,
    ) -> ServiceResult<()> {
        #[derive(Serialize)]
        struct PostData {
            #[serde(rename = "data.TaskScreenName")]
            dataTaskScreenName: String,
            #[serde(rename = "data.LanguageId")]
            dataLanguageId: u32,
            sourceCode: String,
            csrf_token: String,
        }

        for (name, relative_url) in
            extract_task_urls_with_names(self.http_get(&contest.tasks_url())?)?
        {
            if name.to_uppercase() == task.to_uppercase() {
                let task_screen_name = {
                    let regex = Regex::new(r"^.*/([a-z0-9_]+)/?$").unwrap();
                    if let Some(caps) = regex.captures(&relative_url) {
                        caps[1].to_owned()
                    } else {
                        break;
                    }
                };
                let source_code = super::replace_class_name_if_necessary(src_path, "Main")?;
                let csrf_token = {
                    let url = format!("https://beta.atcoder.jp{}", relative_url);
                    extract_csrf_token(self.http_get(&url)?)?
                };
                let data = PostData {
                    dataTaskScreenName: task_screen_name,
                    dataLanguageId: lang_id,
                    sourceCode: source_code,
                    csrf_token: csrf_token,
                };
                let url = contest.submission_url();
                let _ = self.http_post_urlencoded(&url, data, StatusCode::Found)?;
                if open_browser {
                    let url = contest.submissions_url();
                    println!("Opening {} in default browser...", url);
                    webbrowser::open(&url)?;
                }
                return Ok(());
            }
        }
        bail!(ServiceErrorKind::NoSuchProblem(task.to_owned()));
    }

    fn save(self) -> ServiceResult<()> {
        self.0.save_cookie_to_db()
    }
}


enum Contest {
    Practice,
    AbcBefore007(u32),
    Abc(u32),
    ArcBefore019(u32),
    Arc(u32),
    Agc(u32),
    ChokudaiS(u32),
}

impl Contest {
    fn new(s: &str) -> ServiceResult<Self> {
        let regex = Regex::new(r"^\s*([a-zA-Z_]+)(\d\d\d)\s*$").unwrap();
        if let Some(caps) = regex.captures(s) {
            let name = caps[1].to_lowercase();
            let number = caps[2].parse::<u32>().unwrap();
            if number == 0 {
                bail!(ServiceErrorKind::UnsupportedContest(s.to_owned()));
            } else if name == "practice" {
                return Ok(Contest::Practice);
            } else if name == "abc" && number < 7 {
                return Ok(Contest::AbcBefore007(number));
            } else if name == "abc" {
                return Ok(Contest::Abc(number));
            } else if name == "arc" && number < 19 {
                return Ok(Contest::ArcBefore019(number));
            } else if name == "arc" {
                return Ok(Contest::Arc(number));
            } else if name == "agc" {
                return Ok(Contest::Agc(number));
            } else if name == "chokudai_s" || name == "chokudais" {
                return Ok(Contest::ChokudaiS(number));
            }
        }
        bail!(ServiceErrorKind::UnsupportedContest(s.to_owned()));
    }

    fn style(&self) -> SampleCaseStyle {
        match *self {
            Contest::AbcBefore007(_) |
            Contest::ArcBefore019(_) => SampleCaseStyle::Old,
            _ => SampleCaseStyle::New,
        }
    }

    fn top_url(&self) -> String {
        static BASE: &'static str = "https://beta.atcoder.jp/contests/";
        match *self {
            Contest::Practice => format!("{}practice", BASE),
            Contest::AbcBefore007(n) => format!("{}abc{:>03}", BASE, n),
            Contest::Abc(n) => format!("{}abc{:>03}", BASE, n),
            Contest::ArcBefore019(n) => format!("{}arc{:>03}", BASE, n),
            Contest::Arc(n) => format!("{}arc{:>03}", BASE, n),
            Contest::Agc(n) => format!("{}agc{:>03}", BASE, n),
            Contest::ChokudaiS(n) => format!("{}chokudai_s{:>03}", BASE, n),
        }
    }

    fn tasks_url(&self) -> String {
        format!("{}/tasks", self.top_url())
    }

    fn registration_url(&self) -> String {
        format!("{}/register", self.top_url())
    }

    fn submission_url(&self) -> String {
        format!("{}/submit", self.top_url())
    }

    fn submissions_url(&self) -> String {
        format!("{}/submissions/me", self.top_url())
    }
}


enum SampleCaseStyle {
    New,
    Old,
}


fn extract_csrf_token<R: Read>(html: R) -> ServiceResult<String> {
    fn extract(document: Document) -> Option<String> {
        try_opt!(document.find(Attr("name", "csrf_token")).next())
            .attr("value")
            .map(str::to_owned)
    }

    super::quit_on_failure(extract(Document::from_read(html)?), String::is_empty)
}


fn extract_task_urls_with_names<R: Read>(html: R) -> ServiceResult<Vec<(String, String)>> {
    fn extract(document: Document) -> Option<Vec<(String, String)>> {
        let mut names_and_pathes = vec![];
        let predicate = Attr("id", "main-container")
            .child(And(Name("div"), Class("row")))
            .child(And(Name("div"), Class("col-sm-12")))
            .child(And(Name("div"), Class("panel")))
            .child(And(Name("table"), Class("table")))
            .child(Name("tbody"))
            .child(Name("tr"));
        for node in document.find(predicate) {
            let node = try_opt!(node.find(And(Name("td"), Class("text-center"))).next());
            let node = try_opt!(node.find(Name("a")).next());
            let url = try_opt!(node.attr("href")).to_owned();
            let name = try_opt!(node.find(Text).next()).text();
            names_and_pathes.push((name, url));
        }
        Some(names_and_pathes)
    }

    super::quit_on_failure(extract(Document::from_read(html)?), Vec::is_empty)
}


fn extract_cases<R: Read>(html: R, style: &SampleCaseStyle) -> ServiceResult<Cases> {
    let document = Document::from_read(html)?;
    match *style {
        SampleCaseStyle::New => extract_cases_from_new_style(document),
        SampleCaseStyle::Old => unimplemented!(),
    }
}


fn extract_cases_from_new_style(document: Document) -> ServiceResult<Cases> {
    fn try_extracting_from_section(section_node: Node, regex: &Regex) -> Option<String> {
        let title = try_opt!(section_node.find(Name("h3")).next()).text();
        let sample = try_opt!(section_node.find(Name("pre")).next()).text();
        return_none_unless!(regex.is_match(&title));
        Some(sample)
    }

    fn extract_for_lang(
        document: &Document,
        re_input: Regex,
        re_output: Regex,
        lang_class_name: &'static str,
    ) -> Option<Vec<(String, String)>> {
        let predicate = Attr("id", "task-statement")
            .child(And(Name("span"), Class("lang")))
            .child(And(Name("span"), Class(lang_class_name)))
            .child(And(Name("div"), Class("part")))
            .child(Name("section"));
        let (mut samples, mut input_sample) = (vec![], None);
        for node in document.find(predicate) {
            input_sample = if let Some(input_sample) = input_sample {
                let output_sample = try_opt!(try_extracting_from_section(node, &re_output));
                samples.push((output_sample, input_sample));
                None
            } else if let Some(input_sample) = try_extracting_from_section(node, &re_input) {
                Some(input_sample)
            } else {
                None
            };
        }
        return_none_unless!(!samples.is_empty());
        Some(samples)
    }

    fn extract(document: Document) -> Option<Cases> {
        let timelimit = try_opt!(extract_timelimit_as_millis(&document));
        let samples = {
            let re_in_ja = Regex::new(r"^入力例 \d+$").unwrap();
            let re_out_ja = Regex::new(r"^出力例 \d+$").unwrap();
            let re_in_en = Regex::new(r"^Sample Input \d+$").unwrap();
            let re_out_en = Regex::new(r"^Sample Output \d+$").unwrap();
            if let Some(samples) = extract_for_lang(&document, re_in_ja, re_out_ja, "lang-ja") {
                samples
            } else {
                try_opt!(extract_for_lang(&document, re_in_en, re_out_en, "lang-en"))
            }
        };
        Some(Cases::from_text(timelimit, samples))
    }

    super::quit_on_failure(extract(document), Cases::is_empty)
}


fn extract_timelimit_as_millis(document: &Document) -> Option<u64> {
    let re_timelimit = Regex::new(r"^\D*(\d+)\s*sec.*$").unwrap();
    let predicate = Attr("id", "main-container")
        .child(And(Name("div"), Class("row")))
        .child(And(Name("div"), Class("col-sm-12")))
        .child(Name("p"))
        .child(Text);
    let text = try_opt!(document.find(predicate).next()).text();
    let caps = try_opt!(re_timelimit.captures(&text));
    Some(1000 * try_opt!(caps[1].parse::<u64>().ok()))
}
