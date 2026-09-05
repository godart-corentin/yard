use crate::error::Result;
use crate::project::Project;
use crate::state::ProjectState;

pub fn run(project: &Project) -> Result<()> {
    let state = ProjectState::load(&project.state_path)?;
    let head = project.head_revision()?;
    let branch = project.current_branch()?;
    let env_tag = project.current_tag_from_env()?;

    println!("Project: {}", project.name);
    println!("Repo:    {}", project.config.repo.display());
    println!(
        "Branch:  {}",
        if branch.is_empty() {
            "(detached)"
        } else {
            &branch
        }
    );
    println!("HEAD:    {head}");

    match &state.current {
        Some(release) => println!("Current: {} ({})", release.revision, release.tag),
        None => println!("Current: not recorded by Yard"),
    }
    match &state.previous {
        Some(release) => println!("Previous: {} ({})", release.revision, release.tag),
        None => println!("Previous: none"),
    }
    println!("Env tag: {}", env_tag.as_deref().unwrap_or("not set"));

    println!();
    println!("Docker Compose:");
    let compose = project.compose_ps()?;
    if compose.is_empty() {
        println!("(no containers)");
    } else {
        println!("{compose}");
    }
    Ok(())
}
