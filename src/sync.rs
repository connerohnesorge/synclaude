use anyhow::{Context, Result};
use gix::bstr::BStr;
use gix::object::tree::EntryKind;
use gix::refs::transaction::PreviousValue;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::config::Config;

/// Initialize the local sync repo.
/// Creates a local git repo, adds the remote, fetches, and merges any existing data.
pub fn init_repo(config: &Config) -> Result<()> {
    let repo_path = &config.local_repo_path;

    if repo_path.join(".git").exists() {
        info!("Repo already exists at {}", repo_path.display());
        return Ok(());
    }

    std::fs::create_dir_all(repo_path)?;

    // Init a fresh local repo
    let repo = gix::init(repo_path).context("Failed to init git repository")?;

    // Configure the remote
    configure_remote(&repo, &config.remote_url)?;

    // Create an initial empty commit
    create_empty_commit(&repo, "synclaude: initial commit")?;

    // Set up the machine branch
    let branch = config.branch_name();
    switch_branch(&repo, &branch)?;

    // Try to fetch existing data from remote
    match fetch_from_origin(&repo) {
        Ok(_) => {
            info!("Fetched remote data");
            // Re-open to pick up fetched refs
            let repo = gix::open(repo_path).context("Failed to reopen repo")?;
            merge_remote_machine_branches(&repo, &branch)?;
            checkout_head_to_worktree(&repo)?;
        }
        Err(e) => {
            warn!(
                "Could not fetch from remote ({}), will retry on next sync",
                e
            );
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

/// Run a git add + commit + push cycle.
/// Uses gitoxide for staging and committing, CLI only for push (gix 0.80 lacks push protocol).
pub fn commit_and_push(config: &Config, message: &str) -> Result<()> {
    let repo_path = &config.local_repo_path;
    let branch = config.branch_name();

    let repo = gix::open(repo_path).context("Failed to open git repository")?;

    // Ensure we're on the right branch
    switch_branch(&repo, &branch)?;

    // Build tree from worktree and compare with HEAD to detect changes
    let tree_id = write_worktree_as_tree(&repo)?;
    let head = repo.head_commit().context("No HEAD commit")?;
    let head_tree_id = head.tree_id()?.detach();

    if tree_id == head_tree_id {
        info!("No changes to commit");
        return Ok(());
    }

    // Commit
    commit_tree(&repo, message, tree_id, vec![head.id().detach()])?;

    // Push (only operation that still requires CLI — gix 0.80 has no push protocol)
    info!("Pushing to remote branch {}", branch);
    run_git_push(repo_path, &branch)?;

    info!("Push complete");
    Ok(())
}

/// Pull changes from remote machine branches and merge into the local working tree.
pub fn pull_and_merge(config: &Config) -> Result<()> {
    let repo_path = &config.local_repo_path;
    let branch = config.branch_name();

    let repo = gix::open(repo_path).context("Failed to open git repository")?;

    // Fetch using gitoxide
    fetch_from_origin(&repo)?;

    // Ensure we're on our machine branch
    switch_branch(&repo, &branch)?;

    // Merge remote machine branches using gitoxide
    merge_remote_machine_branches(&repo, &branch)?;

    // Update worktree to match the merged HEAD (gix merge only updates the ODB, not the worktree)
    checkout_head_to_worktree(&repo)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Gitoxide helpers
// ---------------------------------------------------------------------------

/// Configure the "origin" remote on a repository.
fn configure_remote(repo: &gix::Repository, url: &str) -> Result<()> {
    let config_path = repo.git_dir().join("config");
    let mut config = gix::config::File::from_path_no_includes(
        config_path.clone(),
        gix::config::Source::Local,
    )
    .unwrap_or_else(|_| {
        gix::config::File::new(gix::config::file::Metadata::from(
            gix::config::Source::Local,
        ))
    });

    config
        .section_mut_or_create_new("remote", Some("origin".into()))
        .context("Failed to create remote section")?
        .set("url".try_into().unwrap(), url.into());

    config
        .section_mut_or_create_new("remote", Some("origin".into()))
        .context("Failed to create remote section")?
        .set(
            "fetch".try_into().unwrap(),
            "+refs/heads/*:refs/remotes/origin/*".into(),
        );

    std::fs::write(&config_path, config.to_bstring())?;
    Ok(())
}

/// Build a default Signature for commits.
fn default_sig() -> gix::actor::Signature {
    gix::actor::Signature {
        name: "synclaude".into(),
        email: "synclaude@localhost".into(),
        time: gix::date::Time::now_local_or_utc(),
    }
}

/// Create a commit with the given tree and parents, using the default synclaude signature.
fn commit_with_sig(
    repo: &gix::Repository,
    message: &str,
    tree_id: gix::ObjectId,
    parents: Vec<gix::ObjectId>,
) -> Result<gix::ObjectId> {
    let sig = default_sig();
    let mut time_buf = gix::date::parse::TimeBuf::default();
    let sig_ref = sig.to_ref(&mut time_buf);

    let commit_id = repo
        .commit_as(sig_ref, sig_ref, "HEAD", message, tree_id, parents)
        .context("Failed to create commit")?;

    Ok(commit_id.detach())
}

/// Create an empty initial commit.
fn create_empty_commit(repo: &gix::Repository, message: &str) -> Result<gix::ObjectId> {
    let empty_tree_id = gix::ObjectId::empty_tree(repo.object_hash());
    commit_with_sig(repo, message, empty_tree_id, vec![])
}

/// Create a commit with the given tree and parents.
fn commit_tree(
    repo: &gix::Repository,
    message: &str,
    tree_id: gix::ObjectId,
    parents: Vec<gix::ObjectId>,
) -> Result<gix::ObjectId> {
    let id = commit_with_sig(repo, message, tree_id, parents)?;
    info!("Created commit {}", id);
    Ok(id)
}

/// Switch HEAD to a branch, creating it if necessary (equivalent to `git checkout -B`).
fn switch_branch(repo: &gix::Repository, branch: &str) -> Result<()> {
    let ref_name = format!("refs/heads/{}", branch);

    // Create/update the branch ref to point at HEAD (if HEAD exists)
    if let Ok(head) = repo.head_commit() {
        repo.reference(
            ref_name.as_str(),
            head.id().detach(),
            PreviousValue::Any,
            format!("synclaude: checkout -B {}", branch),
        )?;
    }

    // Point HEAD symbolically to the branch
    use gix::refs::transaction::{Change, LogChange, RefEdit, RefLog};

    repo.edit_reference(RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: format!("synclaude: switch to {}", branch).into(),
            },
            expected: PreviousValue::Any,
            new: gix::refs::Target::Symbolic(
                gix::refs::FullName::try_from(ref_name.as_str())
                    .map_err(|e| anyhow::anyhow!("{}", e))?,
            ),
        },
        name: gix::refs::FullName::try_from("HEAD")
            .map_err(|e| anyhow::anyhow!("{}", e))?,
        deref: false,
    })?;

    debug!("On branch {}", branch);
    Ok(())
}

/// Fetch from origin using gitoxide's native networking.
/// Supports HTTPS (GitHub, GitLab, Gitea, etc.) and SSH via the user's
/// configured credentials (credential helpers, SSH agent, etc.).
fn fetch_from_origin(repo: &gix::Repository) -> Result<()> {
    let remote = repo
        .find_remote("origin")
        .context("No remote 'origin' configured")?;

    let outcome = remote
        .connect(gix::remote::Direction::Fetch)?
        .prepare_fetch(gix::progress::Discard, Default::default())?
        .receive(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)?;

    debug!(
        "Fetch complete: {} ref mappings",
        outcome.ref_map.mappings.len()
    );
    Ok(())
}

/// Walk the worktree and write all files as a tree object, returning the tree ID.
/// This is the native equivalent of `git add -A && git write-tree`.
fn write_worktree_as_tree(repo: &gix::Repository) -> Result<gix::ObjectId> {
    let workdir = repo
        .workdir()
        .context("Repository has no working directory")?;

    // Start from an empty tree and build up
    let empty_tree = repo.empty_tree();
    let mut editor = empty_tree.edit()?;

    // Walk the worktree recursively, skipping .git
    add_dir_to_tree(repo, &mut editor, workdir, "")?;

    let tree_id = editor.write()?;
    Ok(tree_id.detach())
}

/// Recursively add directory contents to a tree editor.
fn add_dir_to_tree(
    repo: &gix::Repository,
    editor: &mut gix::object::tree::Editor<'_>,
    dir: &Path,
    prefix: &str,
) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip .git directory
        if name_str == ".git" {
            continue;
        }

        let path = if prefix.is_empty() {
            name_str.to_string()
        } else {
            format!("{}/{}", prefix, name_str)
        };

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            add_dir_to_tree(repo, editor, &entry.path(), &path)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            let target_bytes = target.to_string_lossy();
            let blob_id = repo.write_blob(target_bytes.as_bytes())?;
            editor.upsert(&path, EntryKind::Link, blob_id.detach())?;
        } else {
            let contents = std::fs::read(entry.path())?;
            let blob_id = repo.write_blob(&contents)?;
            editor.upsert(&path, EntryKind::Blob, blob_id.detach())?;
        }
    }

    Ok(())
}

/// Write HEAD's tree contents to the worktree.
/// This is necessary after merge/checkout operations since gix only updates the ODB,
/// not the working directory.
fn checkout_head_to_worktree(repo: &gix::Repository) -> Result<()> {
    let workdir = repo
        .workdir()
        .context("Repository has no working directory")?
        .to_path_buf();

    let head = repo.head_commit().context("No HEAD commit")?;
    let tree = head.tree()?;

    // Clean existing tracked content (except .git)
    for entry in std::fs::read_dir(&workdir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }

    // Write tree entries to worktree
    write_tree_to_dir(repo, &tree, &workdir)?;

    Ok(())
}

/// Recursively write a tree object's contents to a directory on disk.
fn write_tree_to_dir(repo: &gix::Repository, tree: &gix::Tree<'_>, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;

    for entry in tree.iter() {
        let entry = entry?;
        let name = entry.filename().to_string();
        let dest = dir.join(&name);
        let mode = entry.mode();

        if mode.is_tree() {
            let subtree = repo.find_object(entry.oid())?.into_tree();
            write_tree_to_dir(repo, &subtree, &dest)?;
        } else if mode.is_link() {
            let blob = repo.find_object(entry.oid())?;
            let target = String::from_utf8_lossy(&blob.data);
            #[cfg(unix)]
            std::os::unix::fs::symlink(target.as_ref(), &dest)?;
        } else if mode.is_blob() {
            let blob = repo.find_object(entry.oid())?;
            std::fs::write(&dest, &blob.data)?;
            #[cfg(unix)]
            if mode.is_executable() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
            }
        } else {
            debug!("Skipping entry {} with mode {:?}", name, mode);
        }
    }

    Ok(())
}

/// Merge all remote machine/* branches using gitoxide's merge API with "theirs" file favor.
fn merge_remote_machine_branches(repo: &gix::Repository, our_branch: &str) -> Result<()> {
    let our_remote = format!("refs/remotes/origin/{}", our_branch);

    // List remote machine branches
    let refs = repo.references()?;
    let remote_refs: Vec<_> = refs
        .prefixed("refs/remotes/origin/machine/")
        .map_err(|e| anyhow::anyhow!("Failed to list refs: {}", e))?
        .filter_map(|r| r.ok())
        .filter(|r| {
            let name = r.name().as_bstr().to_string();
            name != our_remote
        })
        .collect();

    for remote_ref in remote_refs {
        let ref_name = remote_ref.name().as_bstr().to_string();
        info!("Merging {} into {}", ref_name, our_branch);

        match merge_theirs(repo, &remote_ref) {
            Ok(_) => info!("Merge of {} successful", ref_name),
            Err(e) => warn!("Merge of {} had issues: {}", ref_name, e),
        }
    }

    Ok(())
}

/// Merge a remote ref into HEAD using "theirs" strategy for conflicts.
fn merge_theirs(repo: &gix::Repository, their_ref: &gix::Reference<'_>) -> Result<()> {
    let our_commit = repo.head_commit().context("No HEAD commit")?;
    let their_id = their_ref.id();
    let their_commit = their_id.object()?.peel_to_commit()?;

    let our_tree = our_commit.tree_id()?.detach();
    let their_tree = their_commit.tree_id()?.detach();

    // Find merge base
    let ancestor_tree = match repo.merge_base(our_commit.id().detach(), their_commit.id().detach())
    {
        Ok(base_id) => {
            let base_commit = base_id.object()?.peel_to_commit()?;
            base_commit.tree_id()?.detach()
        }
        Err(_) => {
            // No common ancestor (unrelated histories) — use empty tree
            gix::ObjectId::empty_tree(repo.object_hash())
        }
    };

    // Configure merge with "theirs" file favor
    let options = repo
        .tree_merge_options()
        .context("Failed to get merge options")?
        .with_file_favor(Some(gix::merge::tree::FileFavor::Theirs));

    let labels = gix::merge::blob::builtin_driver::text::Labels {
        ancestor: Some(BStr::new("ancestor")),
        current: Some(BStr::new("ours")),
        other: Some(BStr::new("theirs")),
    };

    let outcome = repo.merge_trees(ancestor_tree, our_tree, their_tree, labels, options)?;

    // Write the merged tree
    let mut editor = outcome.tree;
    let merged_tree_id = editor.write()?.detach();

    // Create merge commit with two parents
    let merge_msg = format!(
        "synclaude: merge {}",
        their_ref.name().as_bstr()
    );

    commit_with_sig(
        repo,
        &merge_msg,
        merged_tree_id,
        vec![our_commit.id().detach(), their_commit.id().detach()],
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Git CLI fallback — push only (gix 0.80 has no push/send-pack protocol)
// ---------------------------------------------------------------------------

fn run_git_push(repo_path: &Path, branch: &str) -> Result<()> {
    debug!("git push -u origin {}", branch);
    let output = std::process::Command::new("git")
        .args(["push", "-u", "origin", branch])
        .current_dir(repo_path)
        .output()
        .context("Failed to run git push")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git push failed: {}", stderr);
    }

    Ok(())
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
