use tracing::warn;

use crate::error::{Result, YardError};

use crate::project::Project;
use crate::state::{ProjectState, Release};

pub fn run(project: &Project, revision: Option<&str>) -> Result<()> {
    project.ensure_clean()?;

    let mut state = ProjectState::load(&project.state_path)?;
    let recovering = state.pending.is_some() && revision.is_none();
    let recovering_first = recovering && state.current.is_none();
    let mut current = if recovering_first {
        state
            .previous
            .clone()
            .ok_or_else(|| YardError::NoPreviousRelease(project.name.clone()))?
    } else {
        current_release(project, &state)?
    };
    if current.services.is_empty() {
        current.services = project.release_services(&current.tag)?;
    }

    let mut target = match revision {
        Some(revision) => {
            let resolved = project.resolve_revision(revision)?;
            Release::new(resolved.clone(), Project::tag_for_revision(&resolved))
        }
        None => (if recovering {
            state.current.clone().or_else(|| state.previous.clone())
        } else {
            state.previous.clone()
        })
        .ok_or_else(|| YardError::NoPreviousRelease(project.name.clone()))?,
    };

    if target.services.is_empty() {
        target.services = project.release_services(&target.tag)?;
    }
    target.status = "active".to_owned();

    for service in &target.services {
        if !project.image_ref_exists(&service.image)? {
            return Err(YardError::ImageMissing(service.image.clone()));
        }
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

    let mut pending = target.clone();
    pending.status = "activating".to_owned();
    state.pending = Some(pending);
    state.save(&project.state_path)?;
    project.persist_tag(&target.tag)?;
    let activation = project.activate(&target);

    if let Err(error) = activation {
        warn!(project = %project.name, %error, "rollback failed; restoring previous application image");
        eprintln!(
            "Docker Compose after failure: {}",
            project
                .compose_ps()
                .unwrap_or_else(|error| error.to_string())
        );
        let restore = (|| -> Result<()> {
            project.persist_tag(&current.tag)?;
            project.activate(&current)?;
            Ok(())
        })();
        if let Err(restore_error) = restore {
            warn!(project = %project.name, %restore_error, "failed to restore the original release");
        } else {
            state.pending = None;
            if recovering_first {
                current.status = "active".to_owned();
                state.current = Some(current);
                state.previous = None;
            }
            state.save(&project.state_path)?;
        }
        return Err(error);
    }

    if !recovering {
        current.status = "superseded".to_owned();
        state.previous = Some(current);
    } else if recovering_first {
        state.previous = None;
    }
    state.current = Some(target.clone());
    state.pending = None;
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
