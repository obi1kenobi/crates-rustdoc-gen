use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use duct::cmd;

static BASE_PATH: &str = "/var/tmp/crates-rustdoc-gen/";
static GITHUB_PREFIX: &str = "https://github.com/";
static GITHUB_GIT_PREFIX: &str = "git@github.com:";

fn process_github_repo(repo_path: &str) -> anyhow::Result<()> {
    let org_and_repo_name = repo_path
        .strip_prefix(GITHUB_PREFIX)
        .expect("invalid prefix");
    let git_path = format!("{}{}.git", GITHUB_GIT_PREFIX, org_and_repo_name);

    let repo_name = org_and_repo_name.replace('/', "__");
    let local_repo_path = format!("{}{}", BASE_PATH, repo_name);

    cmd!("git", "clone", git_path, &local_repo_path).run()?;
    cmd!(
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
    .run()?;

    cmd!(
        "mv",
        format!("{}/{}", local_repo_path, "target/doc"),
        format!("./localdata/{}", repo_name),
    ).run()?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let file = File::open("./localdata/top50-repos.txt")?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(2) {
        let line = line?;
        let trimmed_line = line.trim();
        if trimmed_line.starts_with(GITHUB_PREFIX) {
            process_github_repo(trimmed_line)?;
        }
    }

    Ok(())
}
