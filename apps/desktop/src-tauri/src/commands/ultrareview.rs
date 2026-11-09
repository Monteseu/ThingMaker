//! Ultra-Review (see `thingmaker_supervisor::ultrareview`): a panel of models
//! from several providers reviews one change and cross-examines its own
//! findings. Every change to a review reaches the renderer whole, as a
//! `thingmaker://ultra-review` event.

use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};
use thingmaker_supervisor::{
    DesktopError,
    ultrareview::{
        DelegationRunner, Review, StartRequest, UltraReviewService,
        checkout::{self, PullRequest},
        debate::SeatRecord,
        markdown,
        model::{Decision, TargetKind},
    },
};

use super::CommandResult;
use crate::state::AppState;

pub const ULTRA_REVIEW_EVENT: &str = "thingmaker://ultra-review";

/// Starts the service; called once from `setup`, after delegation.
pub fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(delegation) = state.delegation.get().cloned() else {
        tracing::warn!("Ultra-Review is unavailable: delegation did not start");
        return;
    };
    let emitter = app.clone();
    let service = UltraReviewService::new(state.storage.clone(), Arc::new(DelegationRunner { delegation }), state.data_dir.clone(), state.home.clone(), move |review: &Review| {
        let _ = emitter.emit(ULTRA_REVIEW_EVENT, review);
    });
    service.mark_interrupted();
    let _ = state.ultra_review.set(service);
}

fn service(state: &AppState) -> CommandResult<UltraReviewService> {
    state.ultra_review.get().cloned().ok_or_else(|| DesktopError::not_ready("Ultra-Review is unavailable: the team service did not start"))
}

/// A session's reviews, newest first.
#[tauri::command]
pub fn ultra_review_list(host: String, state: State<'_, AppState>) -> CommandResult<Vec<Review>> {
    service(&state)?.list(&host)
}

#[tauri::command]
pub async fn ultra_review_start(request: StartRequest, state: State<'_, AppState>) -> CommandResult<Review> {
    service(&state)?.start(request).await
}

#[tauri::command]
pub async fn ultra_review_cancel(id: String, state: State<'_, AppState>) -> CommandResult<()> {
    service(&state)?.cancel(&id).await
}

#[tauri::command]
pub fn ultra_review_resume(id: String, state: State<'_, AppState>) -> CommandResult<Review> {
    service(&state)?.resume(&id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRequest {
    pub id: String,
    pub finding: String,
    pub text: String,
}

/// The user steps into a finding's thread.
#[tauri::command]
pub fn ultra_review_note(request: NoteRequest, state: State<'_, AppState>) -> CommandResult<Review> {
    service(&state)?.note(&request.id, &request.finding, &request.text)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecideRequest {
    pub id: String,
    pub finding: String,
    #[serde(default)]
    pub decision: Option<Decision>,
}

/// The user's call on a finding the panel split over.
#[tauri::command]
pub fn ultra_review_decide(request: DecideRequest, state: State<'_, AppState>) -> CommandResult<Review> {
    service(&state)?.decide(&request.id, &request.finding, request.decision)
}

#[tauri::command]
pub fn ultra_review_delete(id: String, state: State<'_, AppState>) -> CommandResult<()> {
    service(&state)?.delete(&id)
}

/// The open pull requests of a workspace's repository, through `gh`; with a
/// query, the ones it finds (a number or link finds that one, open or not).
#[tauri::command]
pub async fn ultra_review_pull_requests(workspace_id: String, query: Option<String>, state: State<'_, AppState>) -> CommandResult<Vec<PullRequest>> {
    let service = service(&state)?;
    tauri::async_runtime::spawn_blocking(move || service.pull_requests(&workspace_id, query.as_deref().unwrap_or(""))).await.map_err(|error| DesktopError::io(error.to_string()))?
}

/// The verdict as Markdown, for copying.
#[tauri::command]
pub fn ultra_review_markdown(id: String, state: State<'_, AppState>) -> CommandResult<String> {
    Ok(markdown::render(&service(&state)?.get(&id)?))
}

/// Posts the verdict on the pull request as a review comment, from the
/// user's own `gh` account.
#[tauri::command]
pub async fn ultra_review_post(id: String, state: State<'_, AppState>) -> CommandResult<()> {
    let service = service(&state)?;
    let review = service.get(&id)?;
    let (TargetKind::Pr, Some(number)) = (review.target.kind, review.target.number) else {
        return Err(DesktopError::not_ready("Only a pull request's review can be posted."));
    };
    if review.verdict.is_none() {
        return Err(DesktopError::not_ready("The review has no verdict yet."));
    }
    let root = state.with_storage(|s| s.workspace_get(&review.workspace_id)).map_err(super::storage_error)?.ok_or_else(|| DesktopError::not_ready("the workspace is gone"))?.canonical_root;
    let program = service.gh().ok_or_else(|| DesktopError::not_ready("GitHub CLI (gh) is not installed"))?;
    let body = markdown::render(&review);
    tauri::async_runtime::spawn_blocking(move || checkout::post_review(&program, std::path::Path::new(&root), number, &body)).await.map_err(|error| DesktopError::io(error.to_string()))?
}

/// How each provider, model and lens's findings have held up, across reviews.
#[tauri::command]
pub fn ultra_review_track_record(state: State<'_, AppState>) -> CommandResult<Vec<SeatRecord>> {
    service(&state)?.track_record()
}
