use tracing::warn;

use crate::error::{Result, YardError};
use crate::project::Project;
use crate::state::{ProjectState, Release};

pub fn run(project: &Project) -> Result<()> {
    project.ensure_clean()?;
    project.switch_branch()?;

    let mut state = ProjectState::load(&project.state_path)?;
    if state.pending.is_some() {
        return Err(YardError::Config(
            "an interrupted release is pending; run yard status and yard rollback first".into(),
        ));
    }
    project.check_tag_writable()?;
    let old_revision = project.head_revision()?;
    let old_tag = if let Some(current) = &state.current {
        current.tag.clone()
    } else {
        project
            .current_tag_from_env()?
            .filter(|tag| !tag.trim().is_empty())
            .unwrap_or_else(|| Project::tag_for_revision(&old_revision))
    };

    project.update_branch()?;
    println!("✓ Git update");

    project.run_backup(&mut state)?;
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

    let services = project.release_services(&new_tag)?;
    for service in &services {
        project
            .compose_build(&new_tag, &service.name)
            .map_err(|source| YardError::Service {
                service: {
                    report_runtime(project);
                    service.name.clone()
                },
                source: Box::new(source),
            })?;
    }
    println!("✓ Build");

    project
        .compose_migrate(&new_tag)
        .map_err(|source| YardError::Service {
            service: {
                report_runtime(project);
                project
                    .config
                    .deployment
                    .migration_service
                    .clone()
                    .unwrap_or_default()
            },
            source: Box::new(source),
        })?;
    if project.config.deployment.migration_service.is_some() {
        println!("✓ Migrations");
    }

    let mut old_release = state
        .current
        .clone()
        .or(Some(Release::new(old_revision.clone(), old_tag.clone())));

    if let Some(old) = &mut old_release {
        if old.services.is_empty() {
            old.services = project.release_services(&old.tag)?;
        }
    }

    let new_release = Release::new(new_revision.clone(), new_tag.clone()).with_services(services);
    let mut pending = new_release.clone();
    pending.status = "activating".to_owned();
    if state.current.is_none() {
        state.previous = old_release.clone().map(|mut release| {
            release.status = "superseded".to_owned();
            release
        });
    }
    // Builds may take long enough for a legacy temporary to appear since preflight.
    project.check_tag_writable()?;
    state.pending = Some(pending);
    state.save(&project.state_path)?;

    project.persist_tag(&new_tag)?;
    let activation = project.activate(&new_release);

    if let Err(error) = activation {
        warn!(project = %project.name, %error, "deployment failed; restoring previous application image");
        report_runtime(project);
        let restore = (|| -> Result<()> {
            project.persist_tag(&old_tag)?;
            if let Some(old) = &old_release {
                project.activate(old)?;
            }
            Ok(())
        })();
        if let Err(restore_error) = restore {
            warn!(project = %project.name, %restore_error, "automatic application rollback also failed");
        } else {
            state.pending = None;
            if state.current.is_none() {
                state.current = old_release.clone();
                state.previous = None;
            }
            state.save(&project.state_path)?;
        }
        return Err(error);
    }

    state.previous = old_release.map(|mut release| {
        release.status = "superseded".to_owned();
        release
    });
    state.current = Some(new_release);
    state.pending = None;
    state.save(&project.state_path)?;

    println!();
    println!("Healthy: {} @ {}", project.name, new_tag);
    Ok(())
}

fn report_runtime(project: &Project) {
    eprintln!(
        "Docker Compose after failure: {}",
        project
            .compose_ps()
            .unwrap_or_else(|error| error.to_string())
    );
}
