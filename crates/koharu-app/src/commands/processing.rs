use std::{collections::HashMap, fmt, sync::Arc};

use anyhow::{Context as _, Result};
use koharu_pipeline::{Committer, Progress, RunStatus, StageOutput, StopToken};
use koharu_scene::Snapshot;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, State, ipc::Channel};
use tauri_runtime_cef::CefRuntime;
use uuid::Uuid;

use super::{ChannelExt as _, Error, canvas::CanvasChannel, project::CurrentProject};
use koharu_desktop::Desktop;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, Type)]
#[serde(transparent)]
pub struct JobId(Uuid);

impl JobId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Job {
    pub id: JobId,
    pub state: JobState,
    #[specta(type = f64)]
    pub completed: usize,
    #[specta(type = f64)]
    pub total: usize,
    #[specta(type = f64)]
    pub pages_completed: usize,
    #[specta(type = f64)]
    pub pages_total: usize,
    pub page: Option<koharu_scene::EntityId>,
    pub stage: Option<koharu_pipeline::Stage>,
    pub model: Option<String>,
    pub error: Option<String>,
}

#[derive(Default)]
struct JobProgress {
    completed: usize,
    total: usize,
    pages_completed: usize,
    pages_total: usize,
    stages_per_page: usize,
    completed_stages: HashMap<koharu_scene::EntityId, usize>,
}

impl JobProgress {
    fn start(&mut self, pages: usize, stages: usize) {
        self.completed = 0;
        self.total = pages.saturating_mul(stages);
        self.pages_completed = 0;
        self.pages_total = pages;
        self.stages_per_page = stages;
        self.completed_stages.clear();
    }

    fn complete_stage(&mut self, page: koharu_scene::EntityId) {
        self.completed = self.completed.saturating_add(1).min(self.total);
        let completed = self.completed_stages.entry(page).or_default();
        let previous = *completed;
        *completed = completed.saturating_add(1).min(self.stages_per_page);
        if previous < self.stages_per_page && *completed == self.stages_per_page {
            self.pages_completed = self.pages_completed.saturating_add(1).min(self.pages_total);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Finished,
    Failed,
    Stopped,
}

#[derive(Default)]
pub(crate) struct Processing {
    pub(crate) stops: Mutex<HashMap<JobId, StopToken>>,
    pub(crate) jobs: Mutex<HashMap<JobId, Job>>,
}

#[derive(Default)]
pub(crate) struct JobChannel {
    pub(crate) channel: Mutex<Option<Channel<Job>>>,
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process(
    handle: AppHandle<CefRuntime>,
    scope: koharu_pipeline::Scope,
    operation: koharu_pipeline::Operation,
    project: State<'_, CurrentProject>,
    processing: State<'_, Processing>,
    job_channel: State<'_, JobChannel>,
) -> std::result::Result<JobId, Error> {
    let snapshot = project
        .project
        .lock()
        .await
        .as_ref()
        .context("no project is open")?
        .snapshot();
    start_process(
        handle,
        snapshot,
        scope,
        operation,
        None,
        &processing,
        &job_channel,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_process(
    handle: AppHandle<CefRuntime>,
    snapshot: Snapshot,
    scope: koharu_pipeline::Scope,
    operation: koharu_pipeline::Operation,
    inpainting_mask: Option<koharu_pipeline::InpaintingMask>,
    processing: &Processing,
    job_channel: &JobChannel,
) -> std::result::Result<JobId, Error> {
    let id = JobId::new();
    let stop = StopToken::default();
    {
        let mut stops = processing.stops.lock();
        if !stops.is_empty() {
            return Err(anyhow::anyhow!("another process is already running").into());
        }
        stops.insert(id, stop.clone());
    }
    let job = Job {
        id,
        state: JobState::Running,
        completed: 0,
        total: 0,
        pages_completed: 0,
        pages_total: 0,
        page: None,
        stage: None,
        model: None,
        error: None,
    };
    processing.jobs.lock().insert(id, job.clone());
    job_channel.channel.publish(job);

    let pipeline = handle.state::<koharu_pipeline::Pipeline>().inner().clone();
    let task_handle = handle.clone();
    drop(tokio::spawn(async move {
        let progress = Arc::new(Mutex::new(JobProgress::default()));
        let progress_handle = task_handle.clone();
        let mut request = koharu_pipeline::Request {
            operation,
            scope,
            stop: stop.clone(),
            progress: None,
            inpainting_mask,
        };
        request.progress = Some(Arc::new(move |event| {
            let update = match event {
                Progress::Started { pages, stages } => {
                    let mut progress = progress.lock();
                    progress.start(pages.len(), stages.len());
                    Some((
                        progress.completed,
                        progress.total,
                        progress.pages_completed,
                        progress.pages_total,
                        None,
                        None,
                        None,
                    ))
                }
                Progress::Loading { page, stage, model } => {
                    let progress = progress.lock();
                    Some((
                        progress.completed,
                        progress.total,
                        progress.pages_completed,
                        progress.pages_total,
                        Some(page),
                        Some(stage),
                        Some(model),
                    ))
                }
                Progress::Finished {
                    page, stage, model, ..
                } => {
                    let mut progress = progress.lock();
                    progress.complete_stage(page);
                    Some((
                        progress.completed,
                        progress.total,
                        progress.pages_completed,
                        progress.pages_total,
                        Some(page),
                        Some(stage),
                        Some(model),
                    ))
                }
                Progress::Skipped { page, stage } => {
                    let mut progress = progress.lock();
                    progress.complete_stage(page);
                    Some((
                        progress.completed,
                        progress.total,
                        progress.pages_completed,
                        progress.pages_total,
                        Some(page),
                        Some(stage),
                        None,
                    ))
                }
                Progress::Running { .. } => None,
            };
            if let Some((completed, total, pages_completed, pages_total, page, stage, model)) =
                update
            {
                let job = {
                    let processing = progress_handle.state::<Processing>();
                    let mut jobs = processing.jobs.lock();
                    jobs.get_mut(&id).map(|job| {
                        job.completed = completed;
                        job.total = total;
                        job.pages_completed = pages_completed;
                        job.pages_total = pages_total;
                        job.page = page;
                        job.stage = stage;
                        job.model = model;
                        job.clone()
                    })
                };
                if let Some(job) = job {
                    progress_handle.state::<JobChannel>().channel.publish(job);
                }
            }
        }));

        struct PipelineCommitter {
            handle: AppHandle<CefRuntime>,
        }

        #[async_trait::async_trait]
        impl Committer for PipelineCommitter {
            async fn commit(&mut self, output: StageOutput) -> Result<Snapshot> {
                let (commit, page) = {
                    let projects = self.handle.state::<CurrentProject>();
                    let mut projects = projects.project.lock().await;
                    let project = projects.as_mut().context("no project is open")?;
                    let Some(commit) = project.commit_rebased(output.patch).await? else {
                        return Ok(project.snapshot());
                    };
                    project.record_commit(&commit);
                    let page = project.active_page();
                    (commit, page)
                };
                let snapshot = commit.snapshot.clone();
                let desktop = self.handle.state::<Desktop>();
                desktop.synchronize(&commit.snapshot, page, &commit).await?;
                let canvas = desktop.canvas_state();
                self.handle.state::<CanvasChannel>().channel.publish(canvas);
                Ok(snapshot)
            }
        }

        let mut committer = PipelineCommitter {
            handle: task_handle.clone(),
        };
        let result = pipeline.execute(snapshot, request, &mut committer).await;
        let (stopped, error) = match result {
            Ok(report) => (report.status == RunStatus::Stopped, None),
            Err(error) => {
                tracing::error!(stage = ?error.stage, %error, "processing failed");
                (false, Some(format!("{error:#}")))
            }
        };
        task_handle.state::<Processing>().stops.lock().remove(&id);
        let job = task_handle
            .state::<Processing>()
            .jobs
            .lock()
            .remove(&id)
            .map(|mut job| {
                job.state = if stopped {
                    JobState::Stopped
                } else if error.is_some() {
                    JobState::Failed
                } else {
                    JobState::Finished
                };
                job.error = error;
                job
            });
        if let Some(job) = job {
            task_handle.state::<JobChannel>().channel.publish(job);
        }
    }));
    Ok(id)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn stop_job(
    job: JobId,
    processing: State<'_, Processing>,
) -> std::result::Result<(), Error> {
    let stops = processing.stops.lock();
    let stop = stops
        .get(&job)
        .with_context(|| format!("job {job} is not running"))?;
    stop.stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::JobProgress;
    use koharu_scene::EntityId;

    #[test]
    fn job_progress_counts_pages_after_every_stage_finishes() {
        let first = EntityId::new();
        let second = EntityId::new();
        let mut progress = JobProgress::default();
        progress.start(2, 2);

        progress.complete_stage(first);
        progress.complete_stage(second);
        assert_eq!(progress.completed, 2);
        assert_eq!(progress.pages_completed, 0);

        progress.complete_stage(first);
        assert_eq!(progress.pages_completed, 1);
        progress.complete_stage(second);
        assert_eq!(progress.completed, 4);
        assert_eq!(progress.pages_completed, 2);
    }
}
