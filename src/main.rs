use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    time::Duration,
};

use crates_io_api::{Crate, CratesQuery, Sort};
use duct::cmd;

static BASE_PATH: &str = "/var/tmp/crates-rustdoc-gen/";
static GITHUB_PREFIX: &str = "https://github.com/";
static GITHUB_GIT_PREFIX: &str = "git@github.com:";

fn check_if_git_tag_exists(repo_path: &str, tag_name: &str) -> anyhow::Result<bool> {
    let result = cmd!("git", "tag", "-l", tag_name)
        .dir(repo_path)
        .stderr_null()
        .stdout_capture()
        .run()?;
    let stdout = String::from_utf8_lossy(&result.stdout);
    Ok(stdout.contains(&format!("{tag_name}\n")))
}

fn discover_tag_format(repo_path: &str, crate_name: &str, version: &str) -> Option<String> {
    let tag_formats = [
        version.to_owned(),
        format!("v{version}"),
        format!("{}-{}", crate_name, version),
        format!("{}-v{}", crate_name, version),
    ];
    for tag_name in tag_formats.into_iter() {
        println!(">> checking tag format {tag_name}");
        if let Ok(true) = check_if_git_tag_exists(repo_path, &tag_name) {
            println!(">> matched tag format: {tag_name}");
            return Some(tag_name);
        }
    }

    None
}

fn process_github_repo(
    crate_info: &Crate,
    repo_path: &str,
    check_semver: bool,
) -> anyhow::Result<()> {
    println!("* crate {} *", &crate_info.name);

    let org_and_repo_name = repo_path
        .strip_prefix(GITHUB_PREFIX)
        .expect("invalid prefix")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let git_path = format!("{}{}.git", GITHUB_GIT_PREFIX, org_and_repo_name);

    let repo_name = org_and_repo_name.replace('/', "__");
    let local_repo_path = format!("{}{}", BASE_PATH, repo_name);

    // Have we already downloaded this repo?
    if Path::new(&format!("{}/.git", &local_repo_path)).is_dir() {
        // Yes! Run "git pull" to make sure it's up to date.
        println!(">> refreshing repo {local_repo_path}");
        cmd!("git", "fetch").dir(&local_repo_path).run()?;
    } else {
        // No. Clone it into that directory.
        println!(">> cloning into {local_repo_path}");
        cmd!("git", "clone", git_path, &local_repo_path).run()?;
    }

    // Check out the tag corresponding to the biggest version number.
    // We would have loved to be able to get the "max_stable_version" instead,
    // but that's not something the crates_io_api crate supports.
    let max_version = crate_info.max_version.as_str();
    println!(">> crate {} max version is {max_version}", &crate_info.name);

    let tag_name = match discover_tag_format(&local_repo_path, &crate_info.name, max_version) {
        Some(tag_name) => tag_name,
        None => {
            println!(
                ">> could not find version tag for crate version {} {} in repo {}",
                crate_info.name, max_version, local_repo_path
            );
            return Ok(());
        }
    };

    if check_semver {
        if let Err(e) = attempt_cargo_semver_checks(&local_repo_path, &crate_info.name) {
            println!(
                ">> cargo-semver-checks run failed for crate version
                {} {max_version} in repo {local_repo_path}: {e}",
                crate_info.name,
            )
        }
    }

    // Check out the that version's tag and build the rustdoc JSON.
    cmd!("git", "checkout", tag_name)
        .dir(&local_repo_path)
        .run()?;

    // Usually, explicitly specifying the package to build works fine, so we try that first.
    let explicit_pkg_build = cmd!(
        "cargo",
        "+nightly",
        "rustdoc",
        "--package",
        &crate_info.name,
        "--all-features",
        "--",
        "--document-private-items",
        "-Zunstable-options",
        "--output-format",
        "json"
    )
    .dir(&local_repo_path)
    .stderr_capture()
    .unchecked()
    .run()
    .expect("unchecked run returned error");
    if !explicit_pkg_build.status.success() {
        let stderr = String::from_utf8_lossy(&explicit_pkg_build.stderr);
        if stderr.contains("ambiguous") {
            // Some crates (e.g. `syn` at `github.com/dtolnay/syn`) contain an ambiguous definition
            // of the package by the name we're looking for.
            //
            // In this case, we try to let rustdoc pick a correct option by default, which might work.
            let implicit_pkg_build = cmd!(
                "cargo",
                "+nightly",
                "rustdoc",
                "--all-features",
                "--",
                "--document-private-items",
                "-Zunstable-options",
                "--output-format",
                "json"
            )
            .dir(&local_repo_path)
            .run();

            if implicit_pkg_build.is_err() {
                // Some packages can't be built with `--all-features`, since some of the features
                // may conflict with each other or may require other special configuration.
                // In this case, we try again with only the features enabled by default.
                cmd!(
                    "cargo",
                    "+nightly",
                    "rustdoc",
                    "--",
                    "--document-private-items",
                    "-Zunstable-options",
                    "--output-format",
                    "json"
                )
                .dir(&local_repo_path)
                .run()?;
            }
        } else if stderr.contains("--lib") && stderr.contains("single target") {
            let lib_pkg_build = cmd!(
                "cargo",
                "+nightly",
                "rustdoc",
                "--package",
                &crate_info.name,
                "--lib",
                "--all-features",
                "--",
                "--document-private-items",
                "-Zunstable-options",
                "--output-format",
                "json"
            )
            .dir(&local_repo_path)
            .run();

            if lib_pkg_build.is_err() {
                // Some packages can't be built with `--all-features`, since some of the features
                // may conflict with each other or may require other special configuration.
                // In this case, we try again with only the features enabled by default.
                cmd!(
                    "cargo",
                    "+nightly",
                    "rustdoc",
                    "--package",
                    &crate_info.name,
                    "--lib",
                    "--",
                    "--document-private-items",
                    "-Zunstable-options",
                    "--output-format",
                    "json"
                )
                .dir(&local_repo_path)
                .run()?;
            }
        } else {
            // Some packages can't be built with `--all-features`, since some of the features
            // may conflict with each other or may require other special configuration.
            // In this case, we try again with only the features enabled by default.
            cmd!(
                "cargo",
                "+nightly",
                "rustdoc",
                "--package",
                &crate_info.name,
                "--",
                "--document-private-items",
                "-Zunstable-options",
                "--output-format",
                "json"
            )
            .dir(&local_repo_path)
            .run()?;
        }
    }

    // Rustdoc JSON is output under a filename that replaces underscores with dashes.
    let underscored_crate_name = crate_info.name.as_str().replace('-', "_");
    cmd!(
        "mv",
        format!(
            "{}/{}/{}.json",
            local_repo_path, "target/doc", underscored_crate_name
        ),
        format!(
            "./localdata/rustdocs/{}-{}.json",
            underscored_crate_name, max_version
        ),
    )
    .run()?;

    Ok(())
}

fn attempt_cargo_semver_checks(local_repo_path: &str, package_name: &str) -> anyhow::Result<()> {
    let cargo_semver_checks_bin = "/home/predrag/.scratch/.rustc-target/cargo-semver-check/target/release/cargo-semver-checks";
    cmd!(
        cargo_semver_checks_bin,
        "semver-checks",
        "check-release",
        "--package",
        package_name,
    )
    .dir(local_repo_path)
    .run()?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let check_semver = false;

    let client = crates_io_api::SyncClient::new(
        "crates-rustdoc-gen (obi1kenobi82@gmail.com)",
        Duration::from_secs(1),
    )?;

    let mut query = CratesQuery::builder()
        .page_size(100)
        .sort(Sort::Downloads)
        .build();
    query.set_page(1);

    let mut failures = vec![];

    for crate_info in client.crates(query)?.crates.into_iter() {
        if let Some(repository) = crate_info.repository.as_deref() {
            if repository.starts_with(GITHUB_PREFIX) {
                if let Err(e) = process_github_repo(&crate_info, repository, check_semver) {
                    failures.push((crate_info, e));
                }
            } else {
                println!(
                    "Crate {} has a non-GitHub repository {}",
                    &crate_info.name, &repository
                );
            }
        } else {
            println!("Crate {} doesn't have repo information", &crate_info.name);
        }
    }

    let error_log_file = File::create("./localdata/errors.log")?;
    let mut writer = BufWriter::new(error_log_file);
    for (crate_info, error_info) in failures.into_iter() {
        writeln!(
            &mut writer,
            "*** {} - {} failed:\n{}",
            crate_info.name, crate_info.max_version, error_info
        )?;
    }

    Ok(())
}
