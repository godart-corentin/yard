use tracing::warn;

use crate::error::{Result, YardError};
use crate::health;
use crate::project::Project;
use crate::state::{ProjectState, Release};

pub fn run(project: &Project, revision: Option<&str>) -> Result<()> {
    project.ensure_clean()?;

    let mut state = ProjectState::load(&project.state_path)?;
    let current = current_release(project, &state)?;

    let target = match revision {
        Some(revision) => {
            let resolved = project.resolve_revision(revision)?;
            Release::new(resolved.clone(), Project::tag_for_revision(&resolved))
        }
        None => state
            .previous
            .clone()
            .ok_or_else(|| YardError::NoPreviousRelease(project.name.clone()))?,
    };

    let image_ref = project.image_ref(&target.tag);
    if !project.image_exists(&target.tag)? {
        return Err(YardError::ImageMissing(image_ref));
    }

    println!("Rolling back {}", project.name);
    println!(
        "  from: {} ({})",
        Project::tag_for_revision(&current.revision),
        current.tag
    );
    println!(
        "    to: {} ({})",
        Project::tag_for_revision(&target.revision),
        target.tag
    );
    println!();

    project.run_backup()?;
    println!("✓ Backup");

    project.persist_tag(&target.tag)?;

    let activation = (|| -> Result<()> {
        project.compose_up(&target.tag)?;
        println!("✓ Activate");
        health::wait(&project.config.deployment)?;
        println!("✓ Health check");
        Ok(())
    })();

    if let Err(error) = activation {
        warn!(project = %project.name, %error, "rollback failed; restoring previous application image");
        let restore = (|| -> Result<()> {
            project.persist_tag(&current.tag)?;
            project.compose_up(&current.tag)?;
            health::wait(&project.config.deployment)?;
            Ok(())
        })();
        if let Err(restore_error) = restore {
            warn!(project = %project.name, %restore_error, "failed to restore the original release");
        }
        return Err(error);
    }

    state.previous = Some(current);
    state.current = Some(target.clone());
    state.save(&project.state_path)?;

    println!();
    println!("Healthy: {} @ {}", project.name, target.tag);
    Ok(())
}

fn current_release(project: &Project, state: &ProjectState) -> Result<Release> {
    if let Some(current) = &state.current {
        return Ok(current.clone());
    }
    let revision = project.head_revision()?;
    let tag = project
        .current_tag_from_env()?
        .filter(|tag| !tag.trim().is_empty())
        .unwrap_or_else(|| Project::tag_for_revision(&revision));
    Ok(Release::new(revision, tag))
}
