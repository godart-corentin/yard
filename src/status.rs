use std::time::SystemTime;

use serde::Deserialize;

use crate::error::Result;
use crate::project::Project;
use crate::state::{ProjectState, Release};

#[derive(Debug, Deserialize)]
struct ComposeService {
    #[serde(rename = "Service", default)]
    service: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "State", default)]
    state: String,
    #[serde(rename = "Health", default)]
    health: String,
    #[serde(rename = "Image", default)]
    image: String,
}

pub fn run(project: &Project) -> Result<()> {
    let state = ProjectState::load(&project.state_path)?;
    let head = project.head_revision()?;
    let branch = project.current_branch()?;
    let env_tag = project.current_tag_from_env()?;

    println!("Project: {}", project.name);
    println!(
        "Branch:  {}",
        if branch.is_empty() {
            "(detached)"
        } else {
            &branch
        }
    );
    println!("HEAD:    {}", Project::tag_for_revision(&head));

    println!();
    println!("Release");
    print_release("current", state.current.as_ref());
    print_release("previous", state.previous.as_ref());
    println!("  {:<9} {}", "env", env_tag.as_deref().unwrap_or("not set"));

    println!();
    println!("Backup");
    match project.last_backup()? {
        Some((path, modified)) => {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("(unknown)");
            println!("  {:<9} {:<9} {}", "last", format_age(modified), name);
        }
        None if project
            .config
            .backup
            .as_ref()
            .and_then(|backup| backup.directory.as_ref())
            .is_some() =>
        {
            println!("  {:<9} none", "last");
        }
        None => println!("  {:<9} not configured", "last"),
    }

    println!();
    println!("Docker Compose");
    let services = parse_compose_services(&project.compose_ps()?)?;
    if services.is_empty() {
        println!("  (no containers)");
    } else {
        let service_width = services
            .iter()
            .map(ComposeService::display_name)
            .map(str::len)
            .max()
            .unwrap_or(7)
            .max(7);
        let status_width = services
            .iter()
            .map(ComposeService::display_status)
            .map(str::len)
            .max()
            .unwrap_or(6)
            .max(6);

        for service in &services {
            println!(
                "  {:<service_width$}  {:<status_width$}  {}",
                service.display_name(),
                service.display_status(),
                service.display_image(),
            );
        }
    }

    Ok(())
}

fn print_release(label: &str, release: Option<&Release>) {
    match release {
        Some(release) => println!(
            "  {:<9} {}  {}",
            label,
            Project::tag_for_revision(&release.revision),
            release.tag
        ),
        None => println!("  {label:<9} none"),
    }
}

fn parse_compose_services(output: &str) -> Result<Vec<ComposeService>> {
    let output = output.trim();
    if output.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(services) = serde_json::from_str::<Vec<ComposeService>>(output) {
        return Ok(services);
    }

    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<ComposeService>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

impl ComposeService {
    fn display_name(&self) -> &str {
        if self.service.is_empty() {
            &self.name
        } else {
            &self.service
        }
    }

    fn display_status(&self) -> &str {
        if self.health.is_empty() {
            &self.state
        } else {
            &self.health
        }
    }

    fn display_image(&self) -> &str {
        if self.image.is_empty() {
            "-"
        } else {
            &self.image
        }
    }
}

fn format_age(modified: SystemTime) -> String {
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return "in future".to_owned();
    };
    let seconds = age.as_secs();

    match seconds {
        0..=9 => "just now".to_owned(),
        10..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}
