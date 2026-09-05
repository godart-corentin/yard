use tracing::warn;

use crate::error::Result;
use crate::health;
use crate::project::Project;
use crate::state::{ProjectState, Release};

pub fn run(project: &Project) -> Result<()> {
    project.ensure_clean()?;
    project.switch_branch()?;

    let mut state = ProjectState::load(&project.state_path)?;
    let old_revision = project.head_revision()?;
    let old_tag = project
        .current_tag_from_env()?
        .filter(|tag| !tag.trim().is_empty())
        .unwrap_or_else(|| Project::tag_for_revision(&old_revision));

    project.update_branch()?;
    println!("✓ Git update");

    project.run_backup()?;
    println!("✓ Backup");

    let new_revision = project.head_revision()?;
    let new_tag = Project::tag_for_revision(&new_revision);

    println!();
    println!("Deploying {}", project.name);
    println!(
        "  from: {} ({})",
        Project::tag_for_revision(&old_revision),
        old_tag
    );
    println!(
        "    to: {} ({})",
        Project::tag_for_revision(&new_revision),
        new_tag
    );
    println!();

    project.compose_build(&new_tag)?;
    println!("✓ Build");

    project.compose_migrate(&new_tag)?;
    if project.config.deployment.migration_service.is_some() {
        println!("✓ Migrations");
    }

    let old_release = state
        .current
        .clone()
        .or(Some(Release::new(old_revision.clone(), old_tag.clone())));

    project.persist_tag(&new_tag)?;

    let activation = (|| -> Result<()> {
        project.compose_up(&new_tag)?;
        println!("✓ Activate");
        health::wait(&project.config.deployment)?;
        println!("✓ Health check");
        Ok(())
    })();

    if let Err(error) = activation {
        warn!(project = %project.name, %error, "deployment failed; restoring previous application image");
        let restore = (|| -> Result<()> {
            project.persist_tag(&old_tag)?;
            project.compose_up(&old_tag)?;
            health::wait(&project.config.deployment)?;
            Ok(())
        })();
        if let Err(restore_error) = restore {
            warn!(project = %project.name, %restore_error, "automatic application rollback also failed");
        }
        return Err(error);
    }

    state.previous = old_release;
    state.current = Some(Release::new(new_revision.clone(), new_tag.clone()));
    state.save(&project.state_path)?;

    println!();
    println!("Healthy: {} @ {}", project.name, new_tag);
    Ok(())
}
