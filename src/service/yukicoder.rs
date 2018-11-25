use crate::errors::{ScrapeError, ScrapeResult, ServiceErrorKind, ServiceResult};
use crate::service::downloader::ZipDownloader;
use crate::service::session::HttpSession;
use crate::service::{
    Contest, DownloadProps, PrintTargets as _PrintTargets, ProblemNameConversion, RevelSession,
    Service, SessionProps, SubmitProps, TryIntoDocument as _TryIntoDocument,
};
use crate::terminal::{Term, WriteAnsi as _WriteAnsi};
use crate::testsuite::{InteractiveSuite, SimpleSuite, SuiteFilePath, TestSuite};

use cookie::Cookie;
use failure::ResultExt as _ResultExt;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::{header, multipart, StatusCode};
use select::document::Document;
use select::predicate::{Attr, Predicate as _Predicate, Text};
use serde_derive::Deserialize;

use std::fmt;
use std::io::Write as _Write;
use std::time::Duration;

pub(crate) fn login(sess_props: SessionProps<impl Term>) -> ServiceResult<()> {
    Yukicoder::try_new(sess_props)?.login(true)
}

pub(crate) fn download(
    mut sess_props: SessionProps<impl Term>,
    download_props: DownloadProps<String>,
) -> ServiceResult<()> {
    let download_props = download_props.convert_contest_and_problems(ProblemNameConversion::Upper);
    download_props.print_targets(sess_props.term.stdout())?;
    let timeout = sess_props.timeout;
    Yukicoder::try_new(sess_props)?.download(&download_props, timeout)
}

pub(crate) fn submit(
    mut sess_props: SessionProps<impl Term>,
    submit_props: SubmitProps<String>,
) -> ServiceResult<()> {
    let submit_props = submit_props.convert_contest_and_problem(ProblemNameConversion::Upper);
    submit_props.print_targets(sess_props.term.stdout())?;
    Yukicoder::try_new(sess_props)?.submit(&submit_props)
}

struct Yukicoder<T: Term> {
    term: T,
    session: HttpSession,
    username: Username,
    credential: RevelSession,
}

impl<T: Term> Service for Yukicoder<T> {
    type Term = T;

    fn session_and_term(&mut self) -> (&mut HttpSession, &mut T) {
        (&mut self.session, &mut self.term)
    }
}

impl<T: Term> Yukicoder<T> {
    fn try_new(mut sess_props: SessionProps<T>) -> ServiceResult<Self> {
        let credential = sess_props.credentials.yukicoder.clone();
        let session = sess_props.start_session()?;
        Ok(Self {
            term: sess_props.term,
            session,
            username: Username::None,
            credential,
        })
    }

    fn login(&mut self, assure: bool) -> ServiceResult<()> {
        if let RevelSession::Some(revel_session) = self.credential.clone() {
            if !self.confirm_revel_session(revel_session.as_ref().clone())? {
                return Err(ServiceErrorKind::LoginOnTest.into());
            }
        }
        self.fetch_username()?;
        if self.username.name().is_none() {
            let mut first = true;
            loop {
                if first {
                    if !assure && !self.term.ask_yes_or_no("Login? ", true)? {
                        break;
                    }
                    writeln!(
                        self.stdout(),
                        "\nInput \"REVEL_SESSION\".\n\n\
                         Firefox: sqlite3 ~/path/to/cookies.sqlite 'SELECT value FROM moz_cookies \
                         WHERE baseDomain=\"yukicoder.me\" AND name=\"REVEL_SESSION\"'\n\
                         Chrome: chrome://settings/cookies/detail?site=yukicoder.me&search=cookie\n"
                    )?;
                    self.stdout().flush()?;
                    first = false;
                }
                let revel_session = self.term.prompt_password_stderr("REVEL_SESSION: ")?;
                if self.confirm_revel_session(revel_session)? {
                    break;
                } else {
                    writeln!(self.stderr(), "Wrong \"REVEL_SESSION\".")?;
                    self.stderr().flush()?;
                }
            }
        }
        let username = self.username.clone();
        writeln!(self.stdout(), "Username: {}", username)?;
        self.stdout().flush()?;
        Ok(())
    }

    fn confirm_revel_session(&mut self, revel_session: String) -> ServiceResult<bool> {
        self.session.clear_cookies()?;
        let cookie = Cookie::new("REVEL_SESSION", revel_session);
        self.session.insert_cookie(cookie)?;
        self.fetch_username()?;
        Ok(self.username.name().is_some())
    }

    fn fetch_username(&mut self) -> ServiceResult<()> {
        self.username = self.get("/").recv_html()?.extract_username();
        Ok(())
    }

    fn download(
        &mut self,
        download_props: &DownloadProps<YukicoderContest>,
        timeout: Option<Duration>,
    ) -> ServiceResult<()> {
        let DownloadProps {
            contest,
            problems,
            destinations,
            open_browser,
        } = download_props;
        self.login(false)?;
        let scrape =
            |document: &Document, problem: &str| -> ServiceResult<(TestSuite, SuiteFilePath)> {
                let suite = document.extract_samples()?;
                let path = destinations.scraping(problem)?;
                Ok((suite, path))
            };
        let (mut outputs, mut nos) = (vec![], vec![]);
        match (contest, problems.as_ref()) {
            (YukicoderContest::No, None) => {
                return Err(ServiceErrorKind::PleaseSpecifyProblems.into())
            }
            (YukicoderContest::No, Some(problems)) => {
                let (mut not_found, mut not_public) = (vec![], vec![]);
                for problem in problems {
                    let url = format!("/problems/no/{}", problem);
                    let res = self.get(&url).acceptable(&[200, 404]).send()?;
                    let status = res.status();
                    let document = res.try_into_document()?;
                    let public = document
                        .find(selector!(#content).child(Text))
                        .next()
                        .map_or(true, |t| !t.text().contains("非表示"));
                    if status == StatusCode::NOT_FOUND {
                        not_found.push(problem);
                    } else if !public {
                        not_public.push(problem);
                    } else {
                        let (suite, path) = scrape(&document, problem)?;
                        outputs.push((url, problem.clone(), suite, path));
                        nos.push(problem.clone());
                    }
                }
                let mut stderr = self.stderr();
                if !not_found.is_empty() {
                    stderr.with_reset(|o| writeln!(o.fg(11)?, "Not found: {:?}", not_found))?;
                    stderr.flush()?;
                }
                if !not_public.is_empty() {
                    stderr.with_reset(|o| writeln!(o.fg(11)?, "Not public: {:?}", not_public))?;
                    stderr.flush()?;
                }
            }
            (YukicoderContest::Contest(contest), problems) => {
                let target_problems = self
                    .get(&format!("/contests/{}", contest))
                    .recv_html()?
                    .extract_problems()?;
                for (name, href) in target_problems {
                    if problems.is_none() || problems.as_ref().unwrap().contains(&name) {
                        let document = self.get(&href).recv_html()?;
                        let (suite, path) = scrape(&document, &name)?;
                        outputs.push((href, name.clone(), suite, path));
                        nos.push(name);
                    }
                }
            }
        }
        let nos = self.filter_solved(&nos)?;
        for (_, name, suite, path) in &outputs {
            suite.save(&name, path, self.stdout())?;
        }
        self.stdout().flush()?;
        if !nos.is_empty() {
            static URL_PREF: &str = "https://yukicoder.me/problems/no/";
            static URL_SUF: &str = "/testcase.zip";
            let cookie = self.session.cookies_to_header_value()?;
            ZipDownloader {
                out: self.stdout(),
                url_pref: URL_PREF,
                url_suf: URL_SUF,
                destinations,
                names: &nos,
                timeout,
                cookie,
            }.download()?;
        }
        if *open_browser {
            for (url, _, _, _) in &outputs {
                self.open_in_browser(url)?;
            }
        }
        Ok(())
    }

    fn submit(&mut self, prop: &SubmitProps<YukicoderContest>) -> ServiceResult<()> {
        let SubmitProps {
            contest,
            problem,
            lang_id,
            src_path,
            replacer,
            open_browser,
            skip_checking_if_accepted,
        } = prop;
        self.login(true)?;
        let code = crate::fs::read_to_string(src_path)?;
        let code = match replacer {
            Some(replacer) => replacer.replace_from_local_to_submission(&problem, &code)?,
            None => code,
        };
        let mut url = match contest {
            YukicoderContest::No => format!("/problems/no/{}", problem),
            YukicoderContest::Contest(contest) => self
                .get(&format!("/contests/{}", contest))
                .recv_html()?
                .extract_problems()?
                .into_iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(problem))
                .map(|(_, href)| href)
                .next()
                .ok_or_else(|| ServiceErrorKind::NoSuchProblem(problem.clone()))?,
        };
        url += "/submit";
        let no = {
            static NO: Lazy<Regex> =
                lazy_regex!(r"\A(https://yukicoder\.me)?/problems/no/(\d+)/submit\z");
            NO.captures(&url).map(|caps| caps[2].to_owned())
        };
        if let Some(no) = no {
            if !(self.filter_solved(&[no])?.is_empty() || *skip_checking_if_accepted) {
                return Err(ServiceErrorKind::AlreadyAccepted.into());
            }
        }
        let document = self.get(&url).recv_html()?;
        let token = document.extract_csrf_token_from_submit_page()?;
        let form = multipart::Form::new()
            .text("csrf_token", token)
            .text("lang", lang_id.clone())
            .text("source", code.clone())
            .text("submit", "提出する");
        let url = document.extract_url_from_submit_page()?;
        let res = self.post(&url).send_multipart(form)?;
        let location = match res.headers().get(header::LOCATION) {
            None => None,
            Some(location) => Some(
                location
                    .to_str()
                    .with_context(|_| ServiceErrorKind::ReadHeader(header::LOCATION))?,
            ),
        };
        if let Some(location) = location.as_ref() {
            if location.starts_with("https://yukicoder.me/submissions/") {
                writeln!(self.stdout(), "Success: {:?}", location)?;
                self.stdout().flush()?;
                if *open_browser {
                    self.open_in_browser(location)?;
                }
                return Ok(());
            }
        }
        Err(ServiceErrorKind::SubmissionRejected(
            lang_id.clone(),
            code.len(),
            res.status(),
            location.map(ToOwned::to_owned),
        ).into())
    }

    fn filter_solved<'b>(
        &mut self,
        nos: &'b [impl 'b + AsRef<str>],
    ) -> ServiceResult<Vec<&'b str>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Problem {
            no: u64,
        }

        if let Some(username) = self.username.name().map(ToOwned::to_owned) {
            let url = format!("/api/v1/solved/name/{}", username);
            let solved_nos = self
                .get(&url)
                .send()?
                .json::<Vec<Problem>>()?
                .into_iter()
                .map(|problem| problem.no.to_string())
                .collect::<Vec<_>>();
            Ok(nos
                .iter()
                .map(AsRef::as_ref)
                .filter(|no1| solved_nos.iter().any(|no2| no1 == no2))
                .collect())
        } else {
            Ok(vec![])
        }
    }
}

enum YukicoderContest {
    No,
    Contest(String),
}

impl fmt::Display for YukicoderContest {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            YukicoderContest::No => write!(f, "no"),
            YukicoderContest::Contest(contest) => write!(f, "{}", contest),
        }
    }
}

impl Contest for YukicoderContest {
    fn from_string(s: String) -> Self {
        if s.eq_ignore_ascii_case("no") {
            YukicoderContest::No
        } else {
            YukicoderContest::Contest(s)
        }
    }
}

#[derive(Clone, Debug)]
enum Username {
    None,
    // /public/img/anony.png (for now)
    Yukicoder(String),
    // https://avatars2.githubusercontent.com/...
    Github(String),
    // ?
    ProbablyTwitter(String),
}

impl Username {
    fn name(&self) -> Option<&str> {
        match self {
            Username::None => None,
            Username::Yukicoder(s) | Username::Github(s) | Username::ProbablyTwitter(s) => Some(&s),
        }
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Username::None => write!(f, "<not logged in>"),
            Username::Yukicoder(s) => write!(f, "{} (yukicoder)", s),
            Username::Github(s) => write!(f, "{} (GitHub)", s),
            Username::ProbablyTwitter(s) => write!(f, "{} (probably Twitter)", s),
        }
    }
}

trait Extract {
    fn extract_username(&self) -> Username;
    fn extract_samples(&self) -> ScrapeResult<TestSuite>;
    fn extract_problems(&self) -> ScrapeResult<Vec<(String, String)>>;
    fn extract_csrf_token_from_submit_page(&self) -> ScrapeResult<String>;
    fn extract_url_from_submit_page(&self) -> ScrapeResult<String>;
}

impl Extract for Document {
    fn extract_username(&self) -> Username {
        let extract = || {
            let a = self.find(selector!(#usermenu>a)).next()?;
            let name = a.find(Text).next()?.text();
            let src = a.find(selector!(img)).next()?.attr("src")?;
            Some(if src == "/public/img/anony.png" {
                Username::Yukicoder(name)
            } else if src.starts_with("https://avatars2.githubusercontent.com") {
                Username::Github(name)
            } else {
                Username::ProbablyTwitter(name)
            })
        };
        extract().unwrap_or(Username::None)
    }

    fn extract_samples(&self) -> ScrapeResult<TestSuite> {
        #[derive(Clone, Copy, PartialEq)]
        enum ProblemKind {
            Regular,
            Special,
            Reactive,
        }

        let extract = || {
            static R: Lazy<Regex> = lazy_regex!(
                "\\A / 実行時間制限 : 1ケース (\\d)\\.(\\d{3})秒 / メモリ制限 : \\d+ MB / \
                 (通常|スペシャルジャッジ|リアクティブ)問題.*\n?.*\\z"
            );
            let text = self
                .find(selector!(#content>div).child(Text))
                .map(|text| text.text())
                .nth(1)?;
            let caps = R.captures(&text)?;
            let timelimit = {
                let s = caps[1].parse::<u64>().unwrap();
                let m = caps[2].parse::<u64>().unwrap();
                Duration::from_millis(1000 * s + m)
            };
            let kind = match &caps[3] {
                "通常" => ProblemKind::Regular,
                "スペシャルジャッジ" => ProblemKind::Special,
                "リアクティブ" => ProblemKind::Reactive,
                _ => return None,
            };
            match kind {
                ProblemKind::Regular | ProblemKind::Special => {
                    let mut samples = vec![];
                    for paragraph in
                        self.find(selector!(#content>div.block>div.sample>div.paragraph))
                    {
                        let pres = paragraph
                            .find(selector!(pre).child(Text))
                            .collect::<Vec<_>>();
                        ensure_opt!(pres.len() == 2);
                        let input = pres[0].text();
                        let output = match kind {
                            ProblemKind::Regular => Some(pres[1].text()),
                            ProblemKind::Special => None,
                            ProblemKind::Reactive => unreachable!(),
                        };
                        samples.push((input, output));
                    }
                    let mut suite = SimpleSuite::new(timelimit).cases(samples);
                    if kind == ProblemKind::Special {
                        suite = suite.any();
                    }
                    Some(suite.into())
                }
                ProblemKind::Reactive => Some(InteractiveSuite::new(timelimit).into()),
            }
        };
        extract().ok_or_else(ScrapeError::new)
    }

    fn extract_problems(&self) -> ScrapeResult<Vec<(String, String)>> {
        let extract = || {
            let mut problems = vec![];
            for tr in self.find(selector!(#content>div.left>table.table>tbody>tr)) {
                let name = tr.find(selector!(td)).nth(0)?.text();
                let href = tr
                    .find(selector!(td))
                    .nth(2)?
                    .find(selector!(a))
                    .next()?
                    .attr("href")?
                    .to_owned();
                problems.push((name, href));
            }
            if problems.is_empty() {
                None
            } else {
                Some(problems)
            }
        };
        extract().ok_or_else(ScrapeError::new)
    }

    fn extract_csrf_token_from_submit_page(&self) -> ScrapeResult<String> {
        self.find(
            selector!(#submit_form>input).child(selector!(input).and(Attr("name", "csrf_token"))),
        ).find_map(|input| input.attr("value").map(ToOwned::to_owned))
        .ok_or_else(ScrapeError::new)
    }

    fn extract_url_from_submit_page(&self) -> ScrapeResult<String> {
        self.find(selector!(submit_form))
            .find_map(|form| form.attr("action").map(ToOwned::to_owned))
            .ok_or_else(ScrapeError::new)
    }
}

#[cfg(test)]
mod tests {
    use crate::errors::ServiceResult;
    use crate::service::session::{HttpSession, UrlBase};
    use crate::service::yukicoder::{Extract as _Extract, Username, Yukicoder};
    use crate::service::{self, RevelSession, Service as _Service};
    use crate::terminal::{Term, TermImpl};
    use crate::testsuite::{InteractiveSuite, SimpleSuite, TestSuite};

    use url::Host;

    use std::borrow::Borrow;
    use std::time::Duration;

    #[test]
    fn it_extracts_samples_from_problem1() {
        let _ = env_logger::try_init();
        test_extracting_samples(
            "/problems/no/1",
            SimpleSuite::new(Duration::from_secs(5)).cases(vec![
                ("3\n100\n3\n1 2 1\n2 3 3\n10 90 10\n10 10 50\n", "20\n"),
                ("3\n100\n3\n1 2 1\n2 3 3\n1 100 10\n10 10 50\n", "50\n"),
                (
                    "10\n10\n19\n1 1 2 4 5 1 3 4 6 4 6 4 5 7 8 2 3 4 9\n\
                     3 5 5 5 6 7 7 7 7 8 8 9 9 9 9 10 10 10 10\n\
                     8 6 8 7 6 6 9 9 7 6 9 7 7 8 7 6 6 8 6\n\
                     8 9 10 4 10 3 5 9 3 4 1 8 3 1 3 6 6 10 4\n",
                    "-1\n",
                ),
            ]),
        );
    }

    #[test]
    fn it_extracts_samples_from_problem188() {
        let _ = env_logger::try_init();
        test_extracting_samples("/problems/no/188", SimpleSuite::new(Duration::from_secs(1)));
    }

    #[test]
    fn it_extracts_samples_from_problem192() {
        let _ = env_logger::try_init();
        test_extracting_samples(
            "/problems/no/192",
            SimpleSuite::new(Duration::from_secs(2))
                .any()
                .cases(vec![("101\n", None), ("1000\n", None)]),
        );
    }

    #[test]
    fn it_extracts_samples_from_problem246() {
        let _ = env_logger::try_init();
        test_extracting_samples(
            "/problems/no/246",
            InteractiveSuite::new(Duration::from_secs(2)),
        );
    }

    fn test_extracting_samples(url: &str, expected: impl Into<TestSuite>) {
        let mut yukicoder = start().unwrap();
        let document = yukicoder.get(url).recv_html().unwrap();
        let samples = document.extract_samples().unwrap();
        assert_eq!(expected.into(), samples);
    }

    #[test]
    fn it_extracts_problems_names_and_hrefs_from_yukicoder_open_2015_small() {
        static EXPECTED: &[(&str, &str)] = &[
            ("A", "/problems/no/191"),
            ("B", "/problems/no/192"),
            ("C", "/problems/no/193"),
            ("D", "/problems/no/194"),
            ("E", "/problems/no/195"),
            ("F", "/problems/no/196"),
        ];
        let _ = env_logger::try_init();
        let problems = {
            let mut yukicoder = start().unwrap();
            let document = yukicoder.get("/contests/100").recv_html().unwrap();
            document.extract_problems().unwrap()
        };
        assert_eq!(own_pairs(EXPECTED), problems);
    }

    fn own_pairs<O: Borrow<B>, B: ToOwned<Owned = O> + ?Sized>(pairs: &[(&B, &B)]) -> Vec<(O, O)> {
        pairs
            .iter()
            .map(|(l, r)| ((*l).to_owned(), (*r).to_owned()))
            .collect()
    }

    fn start() -> ServiceResult<Yukicoder<impl Term>> {
        let client = service::reqwest_client(Duration::from_secs(60))?;
        let base = UrlBase::new(Host::Domain("yukicoder.me"), true, None);
        let mut term = TermImpl::null();
        let session = HttpSession::try_new(term.stdout(), client, base, None, true)?;
        Ok(Yukicoder {
            term,
            session,
            username: Username::None,
            credential: RevelSession::None,
        })
    }
}
