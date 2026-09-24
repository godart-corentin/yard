//! Conservative, opt-in inventory of images recorded by Yard releases.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;
use tracing::info;

use crate::command;
use crate::error::{Result, YardError};
use crate::project::Project;
use crate::state::{ProjectState, Release};

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListedImage {
    repository: String,
    tag: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectedImage {
    id: String,
    size: u64,
    repo_tags: Vec<String>,
}

struct Candidate {
    reference: String,
    id: String,
    size: u64,
}

struct Inventory {
    name: String,
    kept: BTreeSet<String>,
    repositories: BTreeSet<String>,
    running: Vec<String>,
    candidates: Vec<Candidate>,
}

fn refused(reason: impl std::fmt::Display) -> YardError {
    YardError::Config(format!("suppression refusée : {reason}"))
}

fn docker(args: &[&str]) -> Result<String> {
    command::checked(
        "docker",
        &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        None,
        &[],
    )
    .map_err(refused)
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix("sha256:")
        .is_some_and(|rest| !rest.is_empty())
}

fn release_images(release: &Release, project: &Project, inventory: &mut Inventory) -> Result<()> {
    if release.tag.is_empty() || release.services.len() != project.config.compose.services.len() {
        return Err(refused(format!(
            "{} : images de release manquantes",
            project.name
        )));
    }
    let mut services = BTreeSet::new();
    for service in &release.services {
        let (repository, tag) = service
            .image
            .rsplit_once(':')
            .ok_or_else(|| refused("référence d'image invalide"))?;
        if repository.is_empty()
            || tag != release.tag
            || !project.config.compose.services.contains(&service.name)
            || !services.insert(&service.name)
        {
            return Err(refused(format!(
                "{} : référence de release incohérente",
                project.name
            )));
        }
        inventory.kept.insert(service.image.clone());
        inventory.repositories.insert(repository.to_owned());
    }
    Ok(())
}

fn running_ids() -> Result<BTreeSet<String>> {
    let output = docker(&["container", "ls", "-q", "--no-trunc"])?;
    let mut running = BTreeSet::new();
    for container in output.lines() {
        if container.trim() != container
            || container.is_empty()
            || container.chars().any(char::is_whitespace)
        {
            return Err(refused("identifiant de conteneur inattendu"));
        }
        let id = docker(&["container", "inspect", "--format", "{{.Image}}", container])?;
        if !valid_id(&id) {
            return Err(refused("image du conteneur non inspectable"));
        }
        running.insert(id);
    }
    Ok(running)
}

fn inspect(reference: &str) -> Result<InspectedImage> {
    let output = docker(&["image", "inspect", reference])?;
    let mut images: Vec<InspectedImage> = serde_json::from_str(&output).map_err(refused)?;
    if images.len() != 1 {
        return Err(refused(format!("inspection d'image ambiguë : {reference}")));
    }
    let image = images.remove(0);
    if !valid_id(&image.id) || !image.repo_tags.iter().any(|tag| tag == reference) {
        return Err(refused(format!(
            "inspection d'image incohérente : {reference}"
        )));
    }
    Ok(image)
}

fn yard_tag(tag: &str) -> bool {
    tag.len() == 12 && tag.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn run(projects_dir: &Path, state_dir: &Path, confirmed: bool) -> Result<()> {
    let mut inventories = BTreeMap::new();
    let mut blocked = Vec::new();
    for name in Project::list(projects_dir)? {
        let result = (|| -> Result<Inventory> {
            let project = Project::load(&name, projects_dir, state_dir)?;
            // An absent state does not prove that no release needs its image.
            if !project.state_path.is_file() {
                return Err(refused("état absent"));
            }
            let state = ProjectState::load(&project.state_path)?;
            if state.pending.is_some() {
                return Err(refused("déploiement en cours"));
            }
            let mut inventory = Inventory {
                name: name.clone(),
                kept: BTreeSet::new(),
                repositories: BTreeSet::new(),
                running: Vec::new(),
                candidates: Vec::new(),
            };
            let current = state
                .current
                .as_ref()
                .ok_or_else(|| refused("release courante absente"))?;
            release_images(current, &project, &mut inventory)?;
            if let Some(previous) = &state.previous {
                release_images(previous, &project, &mut inventory)?;
            }
            Ok(inventory)
        })();
        match result {
            Ok(inventory) => {
                inventories.insert(name, inventory);
            }
            Err(error) => blocked.push(format!("{name} : {error}")),
        }
    }

    // Never infer ownership from a broken manifest or state: no destructive operation
    // may proceed while a configured project cannot be accounted for.
    for warning in &blocked {
        eprintln!("{warning}");
    }
    if !blocked.is_empty() {
        return Err(refused(
            "au moins un projet n'est pas inspectable ; aucune image supprimée",
        ));
    }
    let protected: BTreeSet<_> = inventories
        .values()
        .flat_map(|item| item.kept.iter().cloned())
        .collect();
    let running = running_ids()?;
    let output = docker(&["image", "ls", "--format", "{{json .}}"])?;
    for line in output.lines() {
        let image: ListedImage = serde_json::from_str(line).map_err(refused)?;
        if !yard_tag(&image.tag) {
            continue;
        }
        let reference = format!("{}:{}", image.repository, image.tag);
        if protected.contains(&reference) {
            continue;
        }
        let Some(owner) = inventories
            .values_mut()
            .find(|item| item.repositories.contains(&image.repository))
        else {
            continue;
        };
        if owner
            .candidates
            .iter()
            .any(|candidate| candidate.reference == reference)
        {
            continue;
        }
        let detail = inspect(&reference)?;
        if !running.contains(&detail.id) {
            owner.candidates.push(Candidate {
                reference,
                id: detail.id,
                size: detail.size,
            });
        } else {
            owner.running.push(reference);
        }
    }
    for inventory in inventories.values() {
        println!("Project: {}", inventory.name);
        for reference in &inventory.kept {
            println!("  keep {reference}");
        }
        for reference in &inventory.running {
            println!("  keep {reference} (running container)");
        }
        let mut total = 0u64;
        for candidate in &inventory.candidates {
            println!(
                "  candidate {} ({} bytes)",
                candidate.reference, candidate.size
            );
            total = total.saturating_add(candidate.size);
        }
        println!("  reclaimable: {total} bytes (estimate; shared layers may reduce savings)");
    }
    if !confirmed {
        println!("Simulation only; run yard images --prune --yes to remove candidates.");
        return Ok(());
    }
    for inventory in inventories.values() {
        for candidate in &inventory.candidates {
            // Recheck immediately before each removal. Never force Docker to untag an
            // image being used by a running container, even if it started since inventory.
            if running_ids()?.contains(&candidate.id) {
                println!("  keep {} (running container)", candidate.reference);
                continue;
            }
            docker(&["image", "rm", &candidate.reference])?;
            info!(project = %inventory.name, image = %candidate.reference, "removed unused image");
            println!("  removed {}", candidate.reference);
        }
    }
    Ok(())
}
