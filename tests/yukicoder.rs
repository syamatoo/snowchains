#[allow(dead_code)]
mod service;

use snowchains::app::{App, Opt};
use snowchains::path::AbsPath;
use snowchains::service::ServiceName;
use snowchains::terminal::{AnsiColorChoice, TermImpl};

use failure::Fallible;

#[test]
fn it_logins() -> Fallible<()> {
    fn login(app: App<TermImpl<&[u8], Vec<u8>, Vec<u8>>>) -> snowchains::Result<()> {
        service::login(app, ServiceName::Yukicoder)
    }

    let _ = env_logger::try_init();
    let stdin = format!("{}\n", service::env_var("YUKICODER_REVEL_SESSION")?);
    service::test_in_tempdir("it_logins", &stdin, login)
}

#[test]
fn it_downloads_testcases() -> Fallible<()> {
    fn download(
        app: App<TermImpl<&[u8], Vec<u8>, Vec<u8>>>,
        contest: &str,
        problems: &[&str],
    ) -> snowchains::Result<()> {
        service::download(app, ServiceName::Yukicoder, contest, problems)
    }

    fn confirm_num_cases(wd: &AbsPath, contest: &str, pairs: &[(&str, usize)]) -> Fallible<()> {
        service::confirm_num_cases(wd, ServiceName::Yukicoder, contest, pairs)
    }

    let _ = env_logger::try_init();
    service::test_in_tempdir(
        "it_downloads_test_cases_from_master",
        &format!("Y\n{}\n", service::env_var("YUKICODER_REVEL_SESSION")?),
        |app| -> Fallible<()> {
            static CONTEST: &str = "no";
            let wd = app.working_dir.clone();
            download(app, CONTEST, &["3", "725", "726"])?;
            confirm_num_cases(&wd, CONTEST, &[("3", 31), ("725", 9), ("726", 25)])
        },
    )
}

#[test]
#[ignore]
fn it_submits_to_no_9000() -> Fallible<()> {
    let _ = env_logger::try_init();
    service::test_in_tempdir(
        "it_submits_to_no_9000",
        &format!("Y\n{}\n", service::env_var("YUKICODER_REVEL_SESSION")?),
        |mut app| -> Fallible<()> {
            static CODE: &[u8] = b"Hello World!\n";
            let dir = app.working_dir.join("yukicoder").join("no").join("txt");
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&dir.join("9000.txt"), CODE)?;
            app.run(Opt::Submit {
                open: false,
                force_compile: false,
                only_transpile: false,
                no_judge: true,
                no_check_duplication: false,
                service: Some(ServiceName::Yukicoder),
                contest: Some("no".to_owned()),
                language: Some("text".to_owned()),
                jobs: None,
                color_choice: AnsiColorChoice::Never,
                problem: "9000".to_owned(),
            })
            .map_err(Into::into)
        },
    )
}
