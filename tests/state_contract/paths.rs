use std::{fs, path::Path};

use agent_terminal::paths::{ProjectPaths, find_project_root, project_digest, scope_digest};
use tempfile::TempDir;

use super::common::{TestResult, create_project};

#[test]
fn project_digest_is_stable_lowercase_and_collision_distinguishing() {
    let first = Path::new("/tmp/agent-terminal/project-a");
    let second = Path::new("/tmp/agent-terminal/project-b");

    let first_digest = project_digest(first);

    assert_eq!(first_digest, project_digest(first));
    assert_eq!(first_digest.len(), 24);
    assert!(first_digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(first_digest.bytes().all(|byte| !byte.is_ascii_uppercase()));
    assert_ne!(first_digest, project_digest(second));
}

#[test]
fn canonical_path_aliases_share_project_storage() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let nested = project.join("nested");
    fs::create_dir(&nested)?;
    let alias = nested.join("..");
    let state_root = temp.path().join("state");

    let canonical = ProjectPaths::new(&project, Some(&state_root), "standalone")?;
    let aliased = ProjectPaths::new(&alias, Some(&state_root), "standalone")?;

    assert_eq!(canonical.project_root(), aliased.project_root());
    assert_eq!(canonical.project_dir(), aliased.project_dir());
    Ok(())
}

#[test]
fn project_path_accessors_stay_under_expected_roots() -> TestResult {
    let temp = TempDir::new()?;
    let project = create_project(temp.path(), "project")?;
    let state_root = temp.path().join("state");
    let paths = ProjectPaths::new(&project, Some(&state_root), "standalone")?;
    let scope_root = state_root.join("scopes").join(scope_digest("standalone"));
    let project_dir = scope_root.join("projects").join(project_digest(&project));

    assert_eq!(paths.scope_root(), scope_root);
    assert_eq!(paths.state_root(), paths.scope_root());
    assert_eq!(paths.project_dir(), project_dir);
    assert_eq!(
        paths.zellij_socket_dir(),
        std::env::temp_dir().join(format!("agent-terminal-{}", scope_digest("standalone")))
    );
    assert_eq!(
        paths.bootstrap_lock_file(),
        scope_root.join("bootstrap.lock")
    );
    assert!(paths.state_file().starts_with(paths.project_dir()));
    assert!(paths.lock_file().starts_with(paths.project_dir()));
    assert!(paths.config_file().starts_with(paths.project_dir()));
    assert!(paths.layout_file().starts_with(paths.project_dir()));
    Ok(())
}

#[test]
fn nearest_git_marker_wins_for_nested_directory() -> TestResult {
    let temp = TempDir::new()?;
    let outer = create_project(temp.path(), "outer")?;
    fs::create_dir(outer.join(".git"))?;
    let nested = outer.join("packages").join("app");
    fs::create_dir_all(nested.join(".git"))?;
    let start = nested.join("src").join("deep");
    fs::create_dir_all(&start)?;

    assert_eq!(find_project_root(&start)?, nested);
    Ok(())
}

#[test]
fn project_root_falls_back_to_start_without_git_marker() -> TestResult {
    let temp = TempDir::new()?;
    let start = temp.path().join("plain").join("nested");
    fs::create_dir_all(&start)?;

    assert_eq!(find_project_root(&start)?, start);
    Ok(())
}
