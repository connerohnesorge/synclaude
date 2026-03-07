use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info, warn};

use crate::config::Config;

/// Initialize the local sync repo.
/// Always creates a local git repo, adds the remote, fetches, and merges any existing data.
pub fn init_repo(config: &Config) -> Result<()> {
    let repo_path = &config.local_repo_path;

    if repo_path.join(".git").exists() {
        info!("Repo already exists at {}", repo_path.display());
        return Ok(());
    }

    std::fs::create_dir_all(repo_path)?;

    // Init a fresh local repo
    run_git(repo_path, &["init"])?;
    run_git(repo_path, &["remote", "add", "origin", &config.remote_url])?;

    // Create an initial empty commit so we have a branch to work with
    run_git(
        repo_path,
        &["commit", "--allow-empty", "-m", "synclaude: initial commit"],
    )?;

    // Set up the machine branch
    let branch = config.branch_name();
    run_git(repo_path, &["checkout", "-B", &branch])?;

    // Try to fetch existing data from remote
    match run_git(repo_path, &["fetch", "--all"]) {
        Ok(_) => {
            info!("Fetched remote data");
            // Merge any existing machine branches
            let remote_branches =
                run_git(repo_path, &["branch", "-r", "--list", "origin/machine/*"])
                    .unwrap_or_default();

            let our_remote = format!("origin/{}", branch);
            for remote_ref in remote_branches.lines() {
                let remote_ref = remote_ref.trim();
                if remote_ref.is_empty() || remote_ref == our_remote {
                    continue;
                }
                info!("Merging existing branch {} during init", remote_ref);
                let _ = run_git(
                    repo_path,
                    &[
                        "merge",
                        remote_ref,
                        "--no-edit",
                        "--allow-unrelated-histories",
                        "--strategy-option=theirs",
                    ],
                );
            }
        }
        Err(e) => {
            warn!("Could not fetch from remote ({}), will retry on next sync", e);
        }
    }

    info!("Repo initialized on branch {}", branch);
    Ok(())
}

/// Copy sync dirs from ~/.claude/ into the local repo working tree.
pub fn stage_changes(config: &Config) -> Result<()> {
    let claude_dir = Config::claude_dir()?;
    let repo_path = &config.local_repo_path;

    for dir_name in &config.sync_dirs {
        let src = claude_dir.join(dir_name);
        let dst = repo_path.join(dir_name);

        if src.exists() {
            debug!("Syncing {} -> {}", src.display(), dst.display());
            copy_dir_recursive(&src, &dst)?;
        } else {
            debug!("Source dir {} does not exist, skipping", src.display());
        }
    }

    Ok(())
}

/// Copy files from the local repo working tree back to ~/.claude/.
pub fn apply_pulled_changes(config: &Config) -> Result<()> {
    let claude_dir = Config::claude_dir()?;
    let repo_path = &config.local_repo_path;

    for dir_name in &config.sync_dirs {
        let src = repo_path.join(dir_name);
        let dst = claude_dir.join(dir_name);

        if src.exists() {
            debug!(
                "Applying pulled changes {} -> {}",
                src.display(),
                dst.display()
            );
            copy_dir_recursive(&src, &dst)?;
        }
    }

    Ok(())
}

/// Run a git add + commit + push cycle using the system git binary.
/// We shell out to git for push/pull since gitoxide's networking is still maturing.
pub fn commit_and_push(config: &Config, message: &str) -> Result<()> {
    let repo_path = &config.local_repo_path;
    let branch = config.branch_name();

    // Ensure we're on the right branch
    run_git(repo_path, &["checkout", "-B", &branch])?;

    // Stage all changes
    run_git(repo_path, &["add", "-A"])?;

    // Check if there's anything to commit
    let status = run_git(repo_path, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        info!("No changes to commit");
        return Ok(());
    }

    // Commit
    run_git(repo_path, &["commit", "-m", message])?;

    // Push
    info!("Pushing to remote branch {}", branch);
    run_git(repo_path, &["push", "-u", "origin", &branch])?;

    info!("Push complete");
    Ok(())
}

/// Pull changes from remote machine branches and merge into the local working tree.
pub fn pull_and_merge(config: &Config) -> Result<()> {
    let repo_path = &config.local_repo_path;
    let branch = config.branch_name();

    // Fetch all
    run_git(repo_path, &["fetch", "--all"])?;

    // Ensure we're on our machine branch
    run_git(repo_path, &["checkout", "-B", &branch])?;

    // Find all remote machine/* branches that aren't ours
    let remote_branches = run_git(repo_path, &["branch", "-r", "--list", "origin/machine/*"])?;

    let our_remote = format!("origin/{}", branch);
    for remote_ref in remote_branches.lines() {
        let remote_ref = remote_ref.trim();
        if remote_ref.is_empty() || remote_ref == our_remote {
            continue;
        }

        info!("Merging {} into {}", remote_ref, branch);
        let result = run_git(
            repo_path,
            &[
                "merge",
                remote_ref,
                "--no-edit",
                "--allow-unrelated-histories",
                "--strategy-option=theirs",
            ],
        );

        match result {
            Ok(_) => info!("Merge of {} successful", remote_ref),
            Err(e) => warn!("Merge of {} had issues: {}", remote_ref, e),
        }
    }

    Ok(())
}

fn run_git(repo_path: &Path, args: &[&str]) -> Result<String> {
    debug!("git {}", args.join(" "));
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .context("Failed to run git command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {} failed: {}", args.join(" "), stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }

    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
