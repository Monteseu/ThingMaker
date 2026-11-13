//! Shared Claude/Codex member-resource catalog. Server secrets stay in Rust.
use std::path::{Path, PathBuf};
use serde::Serialize;
use tauri::State;
use thingmaker_supervisor::{DesktopError, agents::{Provider, accounts}, delegation::WorkerSlot,
    integrations::member_resources::{self, ResourceCatalog, ResolvedResources}};
use crate::state::AppState;
use super::{CommandResult, storage_error};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    #[serde(flatten)]
    catalog: ResourceCatalog,
    problem: Option<String>,
}

fn catalog_for(state: &AppState, root: Option<&Path>) -> CommandResult<ResourceCatalog> {
    let home = state.home.as_deref().ok_or_else(|| DesktopError::not_ready("HOME is not set"))?;
    let folder = state.account_folder(Provider::Claude, None).or_else(|| accounts::main_folder(Provider::Claude, home))
        .ok_or_else(|| DesktopError::not_ready("Claude account folder is unavailable"))?;
    let codex_folder = state.account_folder(Provider::Codex, None).or_else(|| accounts::main_folder(Provider::Codex, home))
        .ok_or_else(|| DesktopError::not_ready("Codex account folder is unavailable"))?;
    member_resources::catalog_with_homes(home, &folder, &codex_folder, root)
}

fn check_harness(state: &AppState, provider: Provider) -> CommandResult<()> {
    let resolved = state.resolve_provider(provider)?;
    if provider == Provider::Codex { return Ok(()) }
    let script = resolved.prefix_args.first().ok_or_else(|| DesktopError::not_ready("Claude ACP adapter script is unavailable"))?;
    member_resources::check_harness(Path::new(script))
}

#[tauri::command]
pub fn member_resources_catalog(workspace_id: Option<String>, provider: Provider, state: State<'_, AppState>) -> CommandResult<CatalogView> {
    if !matches!(provider, Provider::Claude | Provider::Codex) { return Err(DesktopError::unsupported("Member resources require Claude Code or Codex")) }
    let root = workspace_id.map(|id| state.with_storage(|s| s.workspace_get(&id)).map_err(storage_error)?
        .map(|record| PathBuf::from(record.canonical_root)).ok_or_else(|| DesktopError::not_ready("Workspace not found"))).transpose()?;
    Ok(CatalogView { catalog: catalog_for(&state, root.as_deref())?.for_provider(provider), problem: check_harness(&state, provider).err().map(|error| error.message) })
}

pub fn resolve(state: &AppState, root: &Path, slot: &WorkerSlot) -> CommandResult<Option<ResolvedResources>> {
    let Some(selection) = &slot.resources else { return Ok(None) };
    if !matches!(slot.provider, Provider::Claude | Provider::Codex) {
        return Err(DesktopError::unsupported("Custom member resources require Claude Code or Codex. Switch provider or restore inherited resources."));
    }
    if selection.skills.len() + selection.mcps.len() > 5000 {
        return Err(DesktopError::limit_exceeded("Too many selected resources"));
    }
    check_harness(state, slot.provider)?;
    let catalog = catalog_for(state, Some(root))?.for_provider(slot.provider);
    let environment = super::session::launch_environment(state).resolve(std::env::vars()).into_iter().collect();
    let resolved = member_resources::resolve(&catalog, selection, &environment)?;
    if resolved.mcps.contains_key(thingmaker_supervisor::delegation::mcp::SERVER_NAME) {
        return Err(DesktopError::conflict("A selected server uses ThingMaker's reserved team-server name"));
    }
    Ok(Some(resolved))
}
