pub mod store;

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast;

use crate::agent::Agent;
use crate::config::SchedulerConfig;
use frankenstein::client_reqwest::Bot;

pub use store::Schedule;
use store::ScheduleStore;

pub struct Scheduler {
    store: ScheduleStore,
    config: SchedulerConfig,
}

impl Scheduler {
    pub async fn new(db_path: &str, config: SchedulerConfig) -> Result<Self> {
        let store = ScheduleStore::new(db_path).await?;
        Ok(Self { store, config })
    }

    pub fn store(&self) -> &ScheduleStore {
        &self.store
    }

    /// Main poll loop. Checks for due jobs and runs them through the agent.
    pub async fn run(
        &self,
        agent: Arc<Agent>,
        bot: Arc<Bot>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<()> {
        let mut interval = tokio::time::interval(
            std::time::Duration::from_secs(self.config.poll_interval_secs),
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!(
            poll_secs = self.config.poll_interval_secs,
            "scheduler started"
        );

        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    tracing::info!("scheduler: shutdown");
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = self.poll_and_run(&agent, &bot).await {
                        tracing::error!(error = %e, "scheduler poll error");
                    }
                }
            }
        }

        Ok(())
    }

    async fn poll_and_run(&self, agent: &Agent, bot: &Bot) -> Result<()> {
        let due_jobs = self.store.get_due_jobs().await?;

        for job in due_jobs {
            tracing::info!(job_id = %job.id, name = %job.name, "running scheduled job");

            let prompt = format!("[scheduled: {}] {}", job.name, job.prompt);

            // Create a throwaway delta channel — we don't stream scheduled messages
            let (delta_tx, mut delta_rx) = tokio::sync::mpsc::channel::<String>(128);
            // Drain the channel in background
            tokio::spawn(async move {
                while delta_rx.recv().await.is_some() {}
            });

            let result = agent
                .process_message(job.chat_id, job.thread_id, "scheduler", &prompt, &[], delta_tx, bot)
                .await;

            match &result {
                Ok(response) => {
                    // Send the response to Telegram
                    let mut params = frankenstein::methods::SendMessageParams::builder()
                        .chat_id(job.chat_id)
                        .text(response)
                        .parse_mode(frankenstein::ParseMode::Html)
                        .build();

                    if let Some(tid) = job.thread_id {
                        params.message_thread_id = Some(tid);
                    }

                    use frankenstein::AsyncTelegramApi;
                    if let Err(e) = bot.send_message(&params).await {
                        let err_str = e.to_string();
                        if err_str.contains("thread not found") && job.thread_id.is_some() {
                            // Thread deleted — create a new one and retry
                            tracing::warn!(job_id = %job.id, name = %job.name, "thread gone, creating new one");
                            let topic_params = frankenstein::methods::CreateForumTopicParams::builder()
                                .chat_id(job.chat_id)
                                .name(&format!("[sched] {}", job.name))
                                .build();
                            match bot.create_forum_topic(&topic_params).await {
                                Ok(topic) => {
                                    let new_tid = topic.result.message_thread_id;
                                    self.store.update_thread_id(&job.id, new_tid).await?;
                                    params.message_thread_id = Some(new_tid);
                                    if let Err(e2) = bot.send_message(&params).await {
                                        tracing::error!(error = %e2, job_id = %job.id, "retry send failed");
                                    }
                                }
                                Err(e2) => {
                                    tracing::error!(error = %e2, job_id = %job.id, "failed to create new thread, disabling job");
                                    self.store.set_enabled(&job.id, false).await?;
                                }
                            }
                        } else if err_str.contains("chat not found") {
                            tracing::warn!(job_id = %job.id, name = %job.name, "chat gone, disabling job");
                            self.store.set_enabled(&job.id, false).await?;
                        } else {
                            tracing::error!(error = %e, job_id = %job.id, "failed to send scheduled message");
                        }
                    }

                    self.store
                        .mark_completed(&job.id, "ok", &truncate(response, 4096))
                        .await?;
                }
                Err(e) => {
                    tracing::error!(error = %e, job_id = %job.id, "scheduled job failed");
                    self.store
                        .mark_completed(&job.id, "error", &e.to_string())
                        .await?;
                }
            }

            // Calculate and set next run (or delete one-shot jobs)
            match &job.schedule {
                Schedule::At { .. } => {
                    // One-shot: delete after execution
                    self.store.delete_job(&job.id).await?;
                    tracing::debug!(job_id = %job.id, "one-shot job deleted");
                }
                schedule => {
                    if let Some(next) = schedule.next_run() {
                        self.store.set_next_run(&job.id, &next).await?;
                    } else {
                        // No more runs — disable
                        self.store.set_enabled(&job.id, false).await?;
                    }
                }
            }
        }

        Ok(())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}
