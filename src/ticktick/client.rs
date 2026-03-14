use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::oauth::TokenStore;

const API_BASE: &str = "https://api.ticktick.com/open/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(rename = "projectId", skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(rename = "dueDate", skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(rename = "sortOrder", default)]
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectData {
    pub project: Project,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

pub struct TickTickClient {
    tokens: TokenStore,
    http: frankenstein::reqwest::Client,
}

impl TickTickClient {
    pub fn new(tokens: TokenStore) -> Self {
        Self {
            tokens,
            http: frankenstein::reqwest::Client::new(),
        }
    }

    pub fn token_store(&self) -> &TokenStore {
        &self.tokens
    }

    async fn auth_header(&self) -> Result<String> {
        let token = self.tokens.get_access_token().await?;
        Ok(format!("Bearer {token}"))
    }

    /// Create a task. If project_id is None, goes to Inbox.
    pub async fn create_task(&self, task: &Task) -> Result<Task> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{API_BASE}/task"))
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(task)?)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("create task failed: {status} — {body}");
        }

        let created: Task = serde_json::from_str(&resp.text().await?)?;
        tracing::debug!(id = ?created.id, title = %created.title, "task created");
        Ok(created)
    }

    /// Update a task.
    #[allow(dead_code)]
    pub async fn update_task(&self, task_id: &str, task: &Task) -> Result<Task> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!("{API_BASE}/task/{task_id}"))
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .body(serde_json::to_string(task)?)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("update task failed: {status} — {body}");
        }

        let updated: Task = serde_json::from_str(&resp.text().await?)?;
        Ok(updated)
    }

    /// Complete a task.
    pub async fn complete_task(&self, project_id: &str, task_id: &str) -> Result<()> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .post(format!(
                "{API_BASE}/project/{project_id}/task/{task_id}/complete"
            ))
            .header("Authorization", &auth)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("complete task failed: {status} — {body}");
        }

        Ok(())
    }

    /// Delete a task.
    pub async fn delete_task(&self, project_id: &str, task_id: &str) -> Result<()> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .delete(format!("{API_BASE}/task/{project_id}/{task_id}"))
            .header("Authorization", &auth)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("delete task failed: {status} — {body}");
        }

        Ok(())
    }

    /// List all projects.
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .get(format!("{API_BASE}/project"))
            .header("Authorization", &auth)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("list projects failed: {status} — {body}");
        }

        let projects: Vec<Project> = serde_json::from_str(&resp.text().await?)?;
        Ok(projects)
    }

    /// Get project with all its tasks.
    pub async fn get_project_data(&self, project_id: &str) -> Result<ProjectData> {
        let auth = self.auth_header().await?;
        let resp = self
            .http
            .get(format!("{API_BASE}/project/{project_id}/data"))
            .header("Authorization", &auth)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("get project data failed: {status} — {body}");
        }

        let data: ProjectData = serde_json::from_str(&resp.text().await?)?;
        Ok(data)
    }

    /// List tasks from all projects.
    pub async fn list_all_tasks(&self) -> Result<Vec<(String, Task)>> {
        let projects = self.list_projects().await?;
        let mut all_tasks = Vec::new();

        for project in &projects {
            match self.get_project_data(&project.id).await {
                Ok(data) => {
                    for task in data.tasks {
                        all_tasks.push((project.name.clone(), task));
                    }
                }
                Err(e) => {
                    tracing::warn!(project = %project.name, error = %e, "failed to get project tasks");
                }
            }
        }

        Ok(all_tasks)
    }
}
